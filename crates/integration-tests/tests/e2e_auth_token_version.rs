//! JWT token_version 失效机制端到端测试
//!
//! 覆盖回归点：`AppState` 构造的 `AuthService` 必须注入带数据库连接的
//! `UserService`，使 `verify_token` 能对 JWT 的 token_version 做数据库比对。
//! 若未正确接线（历史缺陷 B1），密码重置/登出后签发的旧 access token 仍能通过
//! 认证，token_version 失效机制形同虚设。

use integration_tests::common::generate_test_id;
use integration_tests::db::{cleanup_test_data, create_test_pool, create_test_tenant};
use keycompute_auth::{
    AuthService, JwtValidator, LoginService, ProduceAiKeyValidator, UserService,
};
use keycompute_db::{
    CreateUserCredentialRequest, CreateUserRequest, DbRouter, UpdateUserCredentialRequest, User,
    UserCredential, models::user::UpdateUserRequest,
};
use keycompute_types::UserRole;
use std::sync::Arc;

/// 构造一个与生产一致、带数据库连接的 AuthService（含 UserService 用于 token_version 校验）
fn build_auth_service(router: Arc<DbRouter>) -> AuthService {
    let jwt_validator = JwtValidator::new("test-secret", "keycompute");
    AuthService::new(ProduceAiKeyValidator::with_pool(Arc::clone(&router)))
        .with_jwt(jwt_validator)
        .with_user_service(UserService::with_pool(router))
}

/// token_version 递增后，旧 token 必须在 verify_token 处被拒绝
#[tokio::test]
async fn test_verify_token_rejected_after_token_version_bump() {
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("cleanup should succeed");

    let tenant = create_test_tenant(&pool, "tv-verify", &test_id).await;
    let user = User::create(
        &pool,
        &CreateUserRequest {
            tenant_id: tenant.id,
            email: format!("tv-verify-{}@example.com", test_id),
            name: Some("Token Version User".to_string()),
            role: Some(UserRole::User),
        },
    )
    .await
    .expect("user should be created");

    // 新建用户 token_version 默认为 0
    assert_eq!(user.token_version, 0, "新用户 token_version 应为 0");

    let router = DbRouter::single(pool.clone());
    let auth = build_auth_service(Arc::clone(&router));
    let jwt = auth
        .get_jwt_validator()
        .expect("jwt validator configured")
        .clone();

    // 使用用户当前 token_version 签发 token
    let token = jwt
        .generate_token_with_version(user.id, user.tenant_id, &user.role, user.token_version)
        .expect("token should be generated");

    // 1. 递增前：token 有效
    let ctx = auth
        .verify_token(&token)
        .await
        .expect("token should be valid before version bump");
    assert_eq!(ctx.user_id, user.id);
    assert_eq!(ctx.token_version, 0);

    // 2. 递增 token_version（模拟密码重置/登出）
    let new_version = User::increment_token_version(&pool, user.id)
        .await
        .expect("increment should succeed");
    assert_eq!(new_version, 1, "递增后 token_version 应为 1");

    // 3. 递增后：旧 token 必须被拒绝（关键回归断言）
    let result = auth.verify_token(&token).await;
    cleanup_test_data(&pool, &test_id).await.ok();

    let err = result.expect_err("stale token must be rejected after token_version bump");
    assert!(
        err.to_string().contains("invalidated"),
        "错误信息应表明 token 已失效，实际: {err}"
    );
}

/// 无 UserService（无数据库连接）时，verify_token 退化为纯结构性校验，不做 token_version 比对
#[tokio::test]
async fn test_verify_token_without_user_service_skips_version_check() {
    let jwt_validator = JwtValidator::new("test-secret", "keycompute");
    // 注意：未调用 with_user_service，模拟无数据库连接场景
    let auth = AuthService::new(ProduceAiKeyValidator::new()).with_jwt(jwt_validator);

    let jwt = auth.get_jwt_validator().expect("jwt configured").clone();
    // 即便 token 携带非零 token_version，无 UserService 时也不会比对数据库
    let token = jwt
        .generate_token_with_version(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), "user", 42)
        .expect("token generated");

    let ctx = auth
        .verify_token(&token)
        .await
        .expect("without user_service, verification is structural only");
    assert_eq!(ctx.token_version, 42);
}

/// 覆盖模型层新增方法：increment_token_version / find_token_version
#[tokio::test]
async fn test_user_token_version_model_methods() {
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("cleanup should succeed");

    let tenant = create_test_tenant(&pool, "tv-model", &test_id).await;
    let user = User::create(
        &pool,
        &CreateUserRequest {
            tenant_id: tenant.id,
            email: format!("tv-model-{}@example.com", test_id),
            name: Some("TV Model User".to_string()),
            role: Some(UserRole::User),
        },
    )
    .await
    .expect("user should be created");

    // find_token_version 初始为 0
    let v0 = User::find_token_version(&pool, user.id)
        .await
        .expect("query should succeed");
    assert_eq!(v0, Some(0));

    // 连续两次递增，返回值单调递增
    let v1 = User::increment_token_version(&pool, user.id)
        .await
        .expect("first increment");
    let v2 = User::increment_token_version(&pool, user.id)
        .await
        .expect("second increment");
    assert_eq!(v1, 1);
    assert_eq!(v2, 2);

    // find_token_version 反映最新值
    let latest = User::find_token_version(&pool, user.id)
        .await
        .expect("query should succeed");
    assert_eq!(latest, Some(2));

    // 不存在的用户返回 None
    let missing = User::find_token_version(&pool, uuid::Uuid::new_v4())
        .await
        .expect("query should succeed");
    assert_eq!(missing, None);

    cleanup_test_data(&pool, &test_id).await.ok();
}

/// 角色承载授权边界；发生实际角色变化时必须原子递增 token_version，
/// 仅修改名称或重复写入相同角色则不能无故注销用户。
#[tokio::test]
async fn test_role_change_invalidates_existing_jwt_only_when_role_changes() {
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("cleanup should succeed");

    let tenant = create_test_tenant(&pool, "tv-role", &test_id).await;
    let user = User::create(
        &pool,
        &CreateUserRequest {
            tenant_id: tenant.id,
            email: format!("tv-role-{}@example.com", test_id),
            name: Some("Role Token User".to_string()),
            role: Some(UserRole::Admin),
        },
    )
    .await
    .expect("user should be created");
    let router = DbRouter::single(pool.clone());
    let auth = build_auth_service(Arc::clone(&router));
    let jwt = auth
        .get_jwt_validator()
        .expect("jwt validator configured")
        .clone();
    let admin_token = jwt
        .generate_token_with_version(user.id, user.tenant_id, &user.role, user.token_version)
        .expect("admin token should be generated");

    let renamed = user
        .update(
            &pool,
            &UpdateUserRequest {
                name: Some("Renamed Admin".to_string()),
                role: None,
            },
        )
        .await
        .expect("name update should succeed");
    assert_eq!(renamed.token_version, 0);
    auth.verify_token(&admin_token)
        .await
        .expect("name-only update must not invalidate the token");

    let unchanged_role = renamed
        .update(
            &pool,
            &UpdateUserRequest {
                name: None,
                role: Some(keycompute_types::AssignableUserRole::Admin),
            },
        )
        .await
        .expect("same-role update should succeed");
    assert_eq!(unchanged_role.token_version, 0);

    let demoted = unchanged_role
        .update(
            &pool,
            &UpdateUserRequest {
                name: None,
                role: Some(keycompute_types::AssignableUserRole::User),
            },
        )
        .await
        .expect("role update should succeed");
    assert_eq!(demoted.role, UserRole::User.as_str());
    assert_eq!(demoted.token_version, 1);

    let error = auth
        .verify_token(&admin_token)
        .await
        .expect_err("pre-demotion admin token must be invalidated");
    assert!(error.to_string().contains("invalidated"));

    cleanup_test_data(&pool, &test_id).await.ok();
}

/// refresh_token 路径回归：token_version 递增后，旧 token 必须被拒绝刷新。
///
/// refresh-token 为公开路由、不经过 `AuthService::verify_token`，其 token_version
/// 比对读取必须走主库（`write_conn`），否则读副本复制延迟会形成绕过窗口——
/// 攻击者可用密码重置/登出后本应失效的旧 token 刷新出新的有效 token。
#[tokio::test]
async fn test_refresh_token_rejected_after_token_version_bump() {
    let pool = create_test_pool().await;
    let test_id = generate_test_id();
    cleanup_test_data(&pool, &test_id)
        .await
        .expect("cleanup should succeed");

    let tenant = create_test_tenant(&pool, "tv-refresh", &test_id).await;
    let user = User::create(
        &pool,
        &CreateUserRequest {
            tenant_id: tenant.id,
            email: format!("tv-refresh-{}@example.com", test_id),
            name: Some("TV Refresh User".to_string()),
            role: Some(UserRole::User),
        },
    )
    .await
    .expect("user should be created");

    // 创建凭证并标记邮箱已验证——refresh 流程要求凭证存在、未锁定、邮箱已验证。
    let credential = UserCredential::create(
        &pool,
        &CreateUserCredentialRequest {
            user_id: user.id,
            password_hash: "dummy-argon2-hash".to_string(),
        },
    )
    .await
    .expect("credential should be created");
    credential
        .update(
            &pool,
            &UpdateUserCredentialRequest {
                email_verified: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("credential email_verified should be set");

    let router = DbRouter::single(pool.clone());
    let jwt = JwtValidator::new("test-secret", "keycompute");
    let login = LoginService::new(Arc::clone(&router), jwt.clone());

    // 用用户当前 token_version(=0) 签发 token
    let token = jwt
        .generate_token_with_version(user.id, user.tenant_id, &user.role, user.token_version)
        .expect("token should be generated");

    // 1. 递增前：refresh 成功，返回新 token
    let refreshed = login
        .refresh_token(&token)
        .await
        .expect("refresh should succeed before version bump");
    assert_eq!(refreshed.user_id, user.id);

    // 2. 递增 token_version（模拟密码重置/登出）
    let new_version = User::increment_token_version(&pool, user.id)
        .await
        .expect("increment should succeed");
    assert_eq!(new_version, 1);

    // 3. 递增后：旧 token 必须被拒绝刷新（关键回归断言）
    let result = login.refresh_token(&token).await;
    cleanup_test_data(&pool, &test_id).await.ok();

    let err = result.expect_err("stale token must be rejected on refresh after version bump");
    assert!(
        err.to_string().contains("invalidated"),
        "错误信息应表明 token 已失效，实际: {err}"
    );
}
