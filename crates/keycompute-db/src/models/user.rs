use crate::DbError;
use chrono::{DateTime, Utc};
use keycompute_types::{AssignableUserRole, UserRole};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 用户模型
#[derive(Debug, Clone, FromQueryResult, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    /// Token 版本号，用于使已签发的 JWT 失效（密码重置/登出时递增）
    #[serde(default)]
    pub token_version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 创建用户请求
#[derive(Debug, Clone, Deserialize)]
pub struct CreateUserRequest {
    pub tenant_id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub role: Option<UserRole>,
}

/// 更新用户请求
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateUserRequest {
    pub name: Option<String>,
    pub role: Option<AssignableUserRole>,
}

/// 转义 LIKE 通配符
fn escape_like_pattern(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' => escaped.push_str(r"\%"),
            '_' => escaped.push_str(r"\_"),
            '\\' => escaped.push_str(r"\\"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// 用户过滤参数
struct UserFilter {
    tenant_id: Option<Uuid>,
    role: Option<String>,
    search_escaped: Option<String>,
}

impl UserFilter {
    fn new(tenant_id: Option<Uuid>, role: Option<&str>, search: Option<&str>) -> Self {
        let search_escaped = search
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(escape_like_pattern);
        Self {
            tenant_id,
            role: role.map(String::from),
            search_escaped,
        }
    }

    /// 构建 WHERE 子句和参数
    fn build_where_clause(&self) -> (String, Vec<sea_orm::Value>) {
        let mut conditions = Vec::new();
        let mut params: Vec<sea_orm::Value> = Vec::new();

        conditions.push("( $1::uuid IS NULL OR tenant_id = $1 )".to_string());
        params.push(self.tenant_id.into());

        conditions.push("( $2::text IS NULL OR role = $2 )".to_string());
        params.push(self.role.as_deref().into());

        if self.search_escaped.is_some() {
            conditions.push(
                "( $3::text IS NULL OR LOWER(email) LIKE '%' || LOWER($3) || '%' ESCAPE '\\' \
                 OR LOWER(COALESCE(name, '')) LIKE '%' || LOWER($3) || '%' ESCAPE '\\' )"
                    .to_string(),
            );
            params.push(self.search_escaped.as_deref().into());
        } else {
            conditions.push("( $3::text IS NULL )".to_string());
            params.push(sea_orm::Value::String(None));
        }

        let where_clause = format!(" WHERE ({})", conditions.join(") AND ("));
        (where_clause, params)
    }
}

impl User {
    /// 创建新用户
    pub async fn create(
        db: &impl ConnectionTrait,
        req: &CreateUserRequest,
    ) -> Result<User, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO users (tenant_id, email, name, role) VALUES ($1, $2, $3, COALESCE($4, 'user')) RETURNING *"#,
            [
                req.tenant_id.into(),
                req.email.as_str().into(),
                req.name.clone().into(),
                req.role.as_ref().map(|role| role.as_str()).into(),
            ],
        );
        let user = User::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::Other("create failed to return row".to_string()))?;

        Ok(user)
    }

    /// 根据 ID 查找用户
    pub async fn find_by_id(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<User>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM users WHERE id = $1",
            [id.into()],
        );
        let user = User::find_by_statement(stmt).one(db).await?;

        Ok(user)
    }

    /// 根据邮箱查找用户
    pub async fn find_by_email(
        db: &impl ConnectionTrait,
        email: &str,
    ) -> Result<Option<User>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM users WHERE email = $1",
            [email.into()],
        );
        let user = User::find_by_statement(stmt).one(db).await?;

        Ok(user)
    }

    /// 查找租户下的所有用户
    pub async fn find_by_tenant(
        db: &impl ConnectionTrait,
        tenant_id: Uuid,
    ) -> Result<Vec<User>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM users WHERE tenant_id = $1",
            [tenant_id.into()],
        );
        let users = User::find_by_statement(stmt).all(db).await?;

        Ok(users)
    }

    /// 查找所有用户（Admin 全局查询）
    pub async fn find_all(
        db: &impl ConnectionTrait,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<User>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM users ORDER BY created_at DESC LIMIT $1 OFFSET $2",
            [limit.into(), offset.into()],
        );
        let users = User::find_by_statement(stmt).all(db).await?;

        Ok(users)
    }

    /// 查找所有用户（Admin 带过滤 + 分页）
    pub async fn find_all_filtered(
        db: &impl ConnectionTrait,
        tenant_id: Option<Uuid>,
        role: Option<&str>,
        search: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<User>, DbError> {
        let filter = UserFilter::new(tenant_id, role, search);
        let (where_clause, mut params) = filter.build_where_clause();
        let limit_idx = params.len() + 1;
        let offset_idx = limit_idx + 1;
        let sql = format!(
            "SELECT * FROM users{} ORDER BY created_at DESC LIMIT ${} OFFSET ${}",
            where_clause, limit_idx, offset_idx
        );
        params.push(limit.into());
        params.push(offset.into());

        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, &sql, params);
        let users = User::find_by_statement(stmt).all(db).await?;
        Ok(users)
    }

    /// 统计过滤后的用户总数
    pub async fn count_all_filtered(
        db: &impl ConnectionTrait,
        tenant_id: Option<Uuid>,
        role: Option<&str>,
        search: Option<&str>,
    ) -> Result<i64, DbError> {
        let filter = UserFilter::new(tenant_id, role, search);
        let (where_clause, params) = filter.build_where_clause();
        let sql = format!("SELECT COUNT(*) FROM users{}", where_clause);

        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, &sql, params);
        let result = db
            .query_one(stmt)
            .await?
            .ok_or_else(|| DbError::Other("count query failed".to_string()))?;
        let count: i64 = result.try_get_by_index(0).map_err(DbError::DatabaseError)?;
        Ok(count)
    }

    /// 统计用户总数
    pub async fn count_all(db: &impl ConnectionTrait) -> Result<i64, DbError> {
        let stmt = Statement::from_string(
            DbBackend::Postgres,
            "SELECT COUNT(*) FROM users".to_string(),
        );
        let result = db
            .query_one(stmt)
            .await?
            .ok_or_else(|| DbError::Other("count query failed".to_string()))?;
        let count: i64 = result.try_get_by_index(0).map_err(DbError::DatabaseError)?;

        Ok(count)
    }

    /// 批量统计租户用户数量
    pub async fn count_by_tenants(
        db: &impl ConnectionTrait,
        tenant_ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, i64>, DbError> {
        #[derive(FromQueryResult)]
        struct TenantCount {
            tenant_id: Uuid,
            count: i64,
        }

        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT tenant_id, COUNT(*) as count FROM users WHERE tenant_id = ANY($1) GROUP BY tenant_id"#,
            [tenant_ids.to_vec().into()],
        );
        let rows: Vec<TenantCount> = TenantCount::find_by_statement(stmt).all(db).await?;

        Ok(rows.into_iter().map(|r| (r.tenant_id, r.count)).collect())
    }

    /// 更新用户
    pub async fn update(
        &self,
        db: &impl ConnectionTrait,
        req: &UpdateUserRequest,
    ) -> Result<User, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE users
               SET name = COALESCE($1, name),
                   role = COALESCE($2, role),
                   token_version = CASE
                       WHEN $2::text IS NOT NULL AND role IS DISTINCT FROM $2::text
                           THEN token_version + 1
                       ELSE token_version
                   END,
                   updated_at = NOW()
               WHERE id = $3
               RETURNING *"#,
            [
                req.name.clone().into(),
                req.role.as_ref().map(|role| role.as_str()).into(),
                self.id.into(),
            ],
        );
        let user = User::find_by_statement(stmt)
            .one(db)
            .await?
            .ok_or_else(|| DbError::not_found("User", self.id.to_string()))?;

        Ok(user)
    }

    /// 递增用户的 token_version，使该用户已签发的所有 JWT 失效
    ///
    /// 用于密码重置、强制登出等安全场景。返回递增后的新版本号。
    pub async fn increment_token_version(
        db: &impl ConnectionTrait,
        user_id: Uuid,
    ) -> Result<i32, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE users SET token_version = token_version + 1, updated_at = NOW() WHERE id = $1 RETURNING token_version"#,
            [user_id.into()],
        );
        let result = db
            .query_one(stmt)
            .await?
            .ok_or_else(|| DbError::not_found("User", user_id.to_string()))?;
        let version: i32 = result.try_get_by_index(0).map_err(DbError::DatabaseError)?;
        Ok(version)
    }

    /// 轻量查询用户当前的 token_version（仅 SELECT 单列）
    ///
    /// 用于 JWT 失效校验的热路径，避免拉取整行用户数据。
    /// 用户不存在时返回 `None`。
    pub async fn find_token_version(
        db: &impl ConnectionTrait,
        user_id: Uuid,
    ) -> Result<Option<i32>, DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT token_version FROM users WHERE id = $1",
            [user_id.into()],
        );
        match db.query_one(stmt).await? {
            Some(row) => {
                let version: i32 = row.try_get_by_index(0).map_err(DbError::DatabaseError)?;
                Ok(Some(version))
            }
            None => Ok(None),
        }
    }

    /// 删除用户
    pub async fn delete(&self, db: &impl ConnectionTrait) -> Result<(), DbError> {
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM users WHERE id = $1",
            [self.id.into()],
        );
        db.execute(stmt).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_like_pattern_no_special_chars() {
        let result = escape_like_pattern("hello");
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_escape_like_pattern_percent() {
        let result = escape_like_pattern("50%");
        assert_eq!(result, r"50\%");
    }

    #[test]
    fn test_escape_like_pattern_underscore() {
        let result = escape_like_pattern("user_name");
        assert_eq!(result, r"user\_name");
    }

    #[test]
    fn test_escape_like_pattern_backslash() {
        let result = escape_like_pattern(r"a\b");
        assert_eq!(result, r"a\\b");
    }

    #[test]
    fn test_escape_like_pattern_mixed() {
        let result = escape_like_pattern("100%_test\\case");
        assert_eq!(result, r"100\%\_test\\case");
    }

    #[test]
    fn test_escape_like_pattern_empty() {
        let result = escape_like_pattern("");
        assert_eq!(result, "");
    }
}
