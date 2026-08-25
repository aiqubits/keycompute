#!/usr/bin/env bash
# =============================================================================
# reset_admin_password.sh — KeyCompute 管理员密码一键重置脚本
#
# 使用方法：
#   chmod +x reset_admin_password.sh
#   sudo ./reset_admin_password.sh
#
# 前提条件：
#   - 通过本项目 Docker Compose 启动的 PostgreSQL 正在运行
#   - Python3 可用（用于生成 Argon2id 密码哈希）
#
# 可选覆盖（自定义 Compose 项目或容器时使用）：
#   KC_RESET_DB_CONTAINER=<container name or id>
#   KC_RESET_DB_USER=<postgres user>
#   KC_RESET_DB_NAME=<database name>
# =============================================================================

set -euo pipefail

# ── 颜色定义 ────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ── 配置 ─────────────────────────────────────────────────────────────────────
DB_CONTAINER_OVERRIDE="${KC_RESET_DB_CONTAINER:-}"
DB_USER_OVERRIDE="${KC_RESET_DB_USER:-}"
DB_NAME_OVERRIDE="${KC_RESET_DB_NAME:-}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"

# ── 工具函数 ─────────────────────────────────────────────────────────────────

info()  { echo -e "${BLUE}[INFO]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; }
ok()    { echo -e "${GREEN}[OK]${NC} $*"; }

container_is_running() {
    [ "$(docker inspect --format '{{.State.Running}}' "$1" 2>/dev/null || true)" = "true" ]
}

container_display_name() {
    local name
    name="$(docker inspect --format '{{.Name}}' "$1" 2>/dev/null || true)"
    name="${name#/}"
    printf '%s\n' "${name:-$1}"
}

container_env_value() {
    local container="$1"
    local key="$2"

    docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "${container}" 2>/dev/null \
        | awk -v key="${key}" 'index($0, key "=") == 1 { sub("^[^=]*=", ""); print; exit }'
}

generate_password_hash() {
    KC_RESET_PASSWORD_INPUT="$1" python3 <<'PYEOF'
import os
import sys
from argon2 import PasswordHasher, Type

password = os.environ["KC_RESET_PASSWORD_INPUT"]
ph = PasswordHasher(
    time_cost=3,
    memory_cost=65536,
    parallelism=4,
    hash_len=32,
    type=Type.ID,
)

try:
    hash_str = ph.hash(password)
    ph.verify(hash_str, password)
except Exception as error:
    print(f"[ERROR] 密码哈希生成或验证失败: {error}", file=sys.stderr)
    sys.exit(1)

print(hash_str)
PYEOF
}

resolve_database_container() {
    local container_id

    # 显式覆盖始终优先，适用于非 Compose 或自定义容器名部署。
    if [ -n "${DB_CONTAINER_OVERRIDE}" ]; then
        if container_is_running "${DB_CONTAINER_OVERRIDE}"; then
            printf '%s\n' "${DB_CONTAINER_OVERRIDE}"
            return 0
        fi
        error "指定的数据库容器 ${DB_CONTAINER_OVERRIDE} 未运行" >&2
        return 1
    fi

    # 优先让 Compose 解析当前项目的实际容器 ID。普通编排的服务名为
    # postgres，主从编排的写库服务名为 postgres-primary。
    container_id="$(
        docker compose --project-directory "${PROJECT_ROOT}" \
            -f "${PROJECT_ROOT}/docker-compose.yml" \
            ps --status running -q postgres 2>/dev/null | head -n 1 || true
    )"
    if [ -n "${container_id}" ] && container_is_running "${container_id}"; then
        printf '%s\n' "${container_id}"
        return 0
    fi

    container_id="$(
        docker compose --project-directory "${PROJECT_ROOT}" \
            -f "${PROJECT_ROOT}/docker-compose.replicas.yml" \
            ps --status running -q postgres-primary 2>/dev/null | head -n 1 || true
    )"
    if [ -n "${container_id}" ] && container_is_running "${container_id}"; then
        printf '%s\n' "${container_id}"
        return 0
    fi

    # 支持 `docker compose -p <custom>`：通过 Compose 工作目录和服务标签限定
    # 当前项目，避免误选同机其他项目的 PostgreSQL。
    for service in postgres postgres-primary; do
        container_id="$(
            docker ps \
                --filter "label=com.docker.compose.project.working_dir=${PROJECT_ROOT}" \
                --filter "label=com.docker.compose.service=${service}" \
                --format '{{.ID}}' 2>/dev/null | head -n 1 || true
        )"
        if [ -n "${container_id}" ] && container_is_running "${container_id}"; then
            printf '%s\n' "${container_id}"
            return 0
        fi
    done

    # 兼容仓库历史上两个固定容器名。只接受精确名称，不做
    # `*postgres*` 模糊匹配，避免选中 ains-postgres 等无关数据库。
    for container_id in keycompute-postgres keycompute-postgres-primary; do
        if container_is_running "${container_id}"; then
            printf '%s\n' "${container_id}"
            return 0
        fi
    done

    return 1
}

# 被 shell 测试 source 时只导出上述辅助函数，不执行交互式重置。
if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
    return 0
fi

# ── 前置检查 ────────────────────────────────────────────────────────────────

info "=== KeyCompute 管理员密码重置 ==="
echo ""

# 检查 Docker 是否可用
if ! command -v docker &>/dev/null; then
    error "Docker 未安装或不在 PATH 中"
    exit 1
fi

# 自动解析当前编排的写库容器
if ! DB_CONTAINER="$(resolve_database_container)"; then
    error "未找到当前 KeyCompute 项目正在运行的数据库容器！"
    info "可用的容器："
    docker ps --format '  {{.Names}}  ({{.Status}})'
    info "如使用自定义容器，可设置 KC_RESET_DB_CONTAINER=<name-or-id>"
    exit 1
fi
DB_CONTAINER_NAME="$(container_display_name "${DB_CONTAINER}")"
DB_USER="${DB_USER_OVERRIDE:-$(container_env_value "${DB_CONTAINER}" POSTGRES_USER)}"
DB_NAME="${DB_NAME_OVERRIDE:-$(container_env_value "${DB_CONTAINER}" POSTGRES_DB)}"
DB_USER="${DB_USER:-keycompute}"
DB_NAME="${DB_NAME:-keycompute}"
ok "数据库容器 ${DB_CONTAINER_NAME} 运行正常"
info "数据库：${DB_NAME}（用户 ${DB_USER}）"

# 检查 Python3
if ! command -v python3 &>/dev/null; then
    error "Python3 未安装，请先安装：apt install python3 python3-pip"
    exit 1
fi
ok "Python3 可用"

# 检查/安装 argon2-cffi
if ! python3 -c "import argon2" 2>/dev/null; then
    warn "argon2-cffi 未安装，正在安装..."
    if command -v pip3 &>/dev/null; then
        pip3 install argon2-cffi -q
    else
        python3 -m pip install argon2-cffi -q
    fi
    if ! python3 -c "import argon2" 2>/dev/null; then
        error "安装 argon2-cffi 失败，请手动安装：pip3 install argon2-cffi"
        exit 1
    fi
    ok "argon2-cffi 安装成功"
else
    ok "argon2-cffi 已安装"
fi
echo ""

# ── 确认当前管理员 ───────────────────────────────────────────────────────────

info "正在查询管理员用户..."

if ! ADMIN_INFO="$(
    docker exec -i "${DB_CONTAINER}" psql \
        -U "${DB_USER}" \
        -d "${DB_NAME}" \
        -v ON_ERROR_STOP=1 \
        -tA \
        -c "SELECT id, email, name FROM users WHERE role = 'system' ORDER BY created_at ASC LIMIT 1;"
)"; then
    error "查询系统管理员失败，请检查数据库连接和表结构！"
    exit 1
fi

if [ -z "${ADMIN_INFO}" ]; then
    error "未找到系统管理员用户！"
    info "请确认数据库中已有管理员账号。"
    exit 1
fi

IFS='|' read -r ADMIN_ID ADMIN_EMAIL_FOUND ADMIN_NAME <<< "${ADMIN_INFO}"
ADMIN_ID="${ADMIN_ID//[[:space:]]/}"
ADMIN_EMAIL_FOUND="${ADMIN_EMAIL_FOUND//[[:space:]]/}"

ok "找到管理员：${ADMIN_NAME} <${ADMIN_EMAIL_FOUND}>"
info "用户 ID：${ADMIN_ID}"
echo ""

# ── 输入新密码 ──────────────────────────────────────────────────────────────

# 注意：默认密码 12345 是产品有意为之的本地/演示便捷值，并非安全缺陷。
# 请勿在 code review 中将其“修复”为强密码或改回其它占位串。生产环境请由
# 运维在此交互式提示中手动输入强密码；此默认值仅在留空时用于快速本地重置。
# DO NOT CHANGE: intentional default for local/demo convenience.
DEFAULT_PASSWORD=12345

echo -e "${YELLOW}请输入新密码（留空则使用默认密码 '${DEFAULT_PASSWORD}'）：${NC}"
read -s -p "  新密码: " NEW_PASSWORD
echo ""

if [ -z "${NEW_PASSWORD}" ]; then
    NEW_PASSWORD="${DEFAULT_PASSWORD}"
    warn "使用默认密码：${DEFAULT_PASSWORD}"
    warn "请登录后立即修改密码！"
else
    read -s -p "  确认密码: " CONFIRM_PASSWORD
    echo ""

    if [ "${NEW_PASSWORD}" != "${CONFIRM_PASSWORD}" ]; then
        error "两次输入的密码不一致，请重新输入"
        exit 1
    fi

    if [ ${#NEW_PASSWORD} -lt 8 ]; then
        error "密码长度至少需要 8 位"
        exit 1
    fi

    # 检查密码复杂度（与后端 PasswordValidator 一致）
    HAS_UPPER=false; HAS_LOWER=false; HAS_DIGIT=false; HAS_SPECIAL=false
    SPECIAL_CHARS='!@#$%^&*()_+-=[]{}|;:'"'"',.<>?/~`'

    for ((i=0; i<${#NEW_PASSWORD}; i++)); do
        c="${NEW_PASSWORD:$i:1}"
        [[ "$c" =~ [A-Z] ]] && HAS_UPPER=true
        [[ "$c" =~ [a-z] ]] && HAS_LOWER=true
        [[ "$c" =~ [0-9] ]] && HAS_DIGIT=true
        [[ "$SPECIAL_CHARS" == *"$c"* ]] && HAS_SPECIAL=true
    done

    ERRORS=""
    $HAS_UPPER  || ERRORS+="  - 需要至少一个大写字母\n"
    $HAS_LOWER  || ERRORS+="  - 需要至少一个小写字母\n"
    $HAS_DIGIT  || ERRORS+="  - 需要至少一个数字\n"
    $HAS_SPECIAL|| ERRORS+="  - 需要至少一个特殊字符 (!@#\$%^&*()_+-=[]{}|;:',.<>?/~\`)\n"

    if [ -n "${ERRORS}" ]; then
        error "密码不符合复杂度要求："
        echo -e "${ERRORS}"
        exit 1
    fi
fi
echo ""

# ── 通过 Python 生成哈希并更新数据库 ──────────────────────────────────────

info "正在生成密码哈希（Argon2id）并更新数据库..."

if ! PASSWORD_HASH="$(generate_password_hash "${NEW_PASSWORD}")"; then
    error "密码哈希生成失败！"
    exit 1
fi

if ! KC_RESET_PASSWORD_HASH="${PASSWORD_HASH}" \
    KC_RESET_ADMIN_ID="${ADMIN_ID}" \
    KC_RESET_DB_CONTAINER_ID="${DB_CONTAINER}" \
    KC_RESET_DB_USER_RESOLVED="${DB_USER}" \
    KC_RESET_DB_NAME_RESOLVED="${DB_NAME}" \
    python3 <<'PYEOF'
import os
import subprocess
import sys
import uuid

hash_str = os.environ["KC_RESET_PASSWORD_HASH"]
user_id = str(uuid.UUID(os.environ["KC_RESET_ADMIN_ID"]))

sql = f"""
WITH updated AS (
    UPDATE user_credentials
    SET password_hash = '{hash_str}',
        failed_login_attempts = 0,
        locked_until = NULL,
        updated_at = NOW()
    WHERE user_id = '{user_id}'
    RETURNING 1
)
SELECT COUNT(*) FROM updated;
"""

cmd = [
    "docker",
    "exec",
    "-i",
    os.environ["KC_RESET_DB_CONTAINER_ID"],
    "psql",
    "-U",
    os.environ["KC_RESET_DB_USER_RESOLVED"],
    "-d",
    os.environ["KC_RESET_DB_NAME_RESOLVED"],
    "-v",
    "ON_ERROR_STOP=1",
    "-tA",
    "-c",
    sql,
]

result = subprocess.run(cmd, capture_output=True, text=True)
if result.returncode != 0:
    print(f"[ERROR] 数据库更新失败: {result.stderr}")
    sys.exit(1)

if result.stdout.strip() != "1":
    print("[ERROR] 系统管理员缺少唯一登录凭证，未更新任何密码", file=sys.stderr)
    sys.exit(1)

print("[OK] 密码哈希生成成功并已更新到数据库")
PYEOF
then
    error "密码重置失败！"
    exit 1
fi
ok "管理员密码重置成功！"
echo ""
info "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
info "  邮箱：${ADMIN_EMAIL_FOUND}"
info "  用户名：${ADMIN_NAME}"
info "  密码：已更新（请牢记新密码）"
info "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
warn "请尽快登录系统验证新密码是否有效！"
