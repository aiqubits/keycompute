#!/usr/bin/env bash
# =============================================================================
# reset_admin_password.sh — KeyCompute 管理员密码一键重置脚本
#
# 使用方法：
#   chmod +x reset_admin_password.sh
#   sudo ./reset_admin_password.sh
#
# 前提条件：
#   - Docker 容器 keycompute-postgres-primary 正在运行
#   - Python3 可用（用于生成 Argon2id 密码哈希）
# =============================================================================

set -euo pipefail

# ── 颜色定义 ────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ── 配置 ─────────────────────────────────────────────────────────────────────
ADMIN_EMAIL="${KC__DEFAULT_ADMIN_EMAIL:-admin@keycompute.local}"
DB_CONTAINER="keycompute-postgres-primary"
DB_USER="keycompute"
DB_NAME="keycompute"

# ── 工具函数 ─────────────────────────────────────────────────────────────────

info()  { echo -e "${BLUE}[INFO]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; }
ok()    { echo -e "${GREEN}[OK]${NC} $*"; }

# ── 前置检查 ────────────────────────────────────────────────────────────────

info "=== KeyCompute 管理员密码重置 ==="
echo ""

# 检查 Docker 是否可用
if ! command -v docker &>/dev/null; then
    error "Docker 未安装或不在 PATH 中"
    exit 1
fi

# 检查数据库容器是否运行
if ! docker ps --format '{{.Names}}' | grep -q "^${DB_CONTAINER}$"; then
    error "数据库容器 ${DB_CONTAINER} 未运行！"
    info "可用的容器："
    docker ps --format '  {{.Names}}  ({{.Status}})'
    exit 1
fi
ok "数据库容器 ${DB_CONTAINER} 运行正常"

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

ADMIN_INFO=$(docker exec -i "${DB_CONTAINER}" psql -U "${DB_USER}" -d "${DB_NAME}" -tA \
    -c "SELECT id, email, name FROM users WHERE email = '${ADMIN_EMAIL}';" 2>/dev/null || true)

if [ -z "${ADMIN_INFO}" ]; then
    # 如果没有找到配置的邮箱，尝试查找 role=system 的用户
    ADMIN_INFO=$(docker exec -i "${DB_CONTAINER}" psql -U "${DB_USER}" -d "${DB_NAME}" -tA \
        -c "SELECT id, email, name FROM users WHERE role = 'system' ORDER BY created_at ASC LIMIT 1;" 2>/dev/null || true)
fi

if [ -z "${ADMIN_INFO}" ]; then
    error "未找到系统管理员用户！"
    info "请确认数据库中已有管理员账号。"
    exit 1
fi

ADMIN_ID=$(echo "${ADMIN_INFO}" | cut -d'|' -f1 | tr -d ' ')
ADMIN_EMAIL_FOUND=$(echo "${ADMIN_INFO}" | cut -d'|' -f2 | tr -d ' ')
ADMIN_NAME=$(echo "${ADMIN_INFO}" | cut -d'|' -f3 | tr -d ' ')

ok "找到管理员：${ADMIN_NAME} <${ADMIN_EMAIL_FOUND}>"
info "用户 ID：${ADMIN_ID}"
echo ""

# ── 输入新密码 ──────────────────────────────────────────────────────────────

DEFAULT_PASSWORD=change-me-admin-password

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

python3 << PYEOF
import subprocess, sys
from argon2 import PasswordHasher, Type

password = """${NEW_PASSWORD}"""
user_id = """${ADMIN_ID}"""

ph = PasswordHasher(
    time_cost=3,
    memory_cost=65536,
    parallelism=4,
    hash_len=32,
    type=Type.ID
)

try:
    hash_str = ph.hash(password)
    # 立即验证生成的哈希
    ph.verify(hash_str, password)
except Exception as e:
    print(f"[ERROR] 密码哈希生成或验证失败: {e}")
    sys.exit(1)

sql = f"""
UPDATE user_credentials
SET password_hash = '{hash_str}',
    failed_login_attempts = 0,
    locked_until = NULL,
    updated_at = NOW()
WHERE user_id = '{user_id}';
"""

cmd = [
    "docker", "exec", "-i",
    "${DB_CONTAINER}",
    "psql", "-U", "${DB_USER}", "-d", "${DB_NAME}",
    "-c", sql
]

result = subprocess.run(cmd, capture_output=True, text=True)
if result.returncode != 0:
    print(f"[ERROR] 数据库更新失败: {result.stderr}")
    sys.exit(1)
else:
    print(f"[OK] 密码哈希生成成功并已更新到数据库")
    print(result.stdout, end="")
PYEOF

if [ $? -ne 0 ]; then
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
