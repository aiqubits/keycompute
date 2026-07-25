#!/usr/bin/env bash
# ============================================================================
# KeyCompute 图片理解 API 调用示例 - curl
# 演示如何使用 curl 调用支持多模态（图片+文本）的 chat 接口
#
# 用法:
#   chmod +x curl_vision.sh
#   ./curl_vision.sh                                    # 使用默认图片 URL
#   ./curl_vision.sh "图片URL" "问题文本"                # 自定义图片和问题
#
# 自定义参数 (环境变量):
#   KC_BASE_URL     服务端地址          (默认: http://localhost:3000)
#   KC_API_KEY      API Key            (默认: sk-your-api-key-here)
#   KC_MODEL        模型名称           (默认: gemma3:4b)
# ============================================================================
# curl --noproxy POST "http://192.168.100.100:3000/v1/chat/completions" \
#  -H "Authorization: Bearer sk-d52b13ed344743eb9d965a6cf760bf0a36d0df35ae054a78" \
#  -H "Content-Type: application/json" \
#  -d '{
#    "model": "gemma3:4b",
#    "messages": [
#      {
#        "role": "user",
#        "content": [
#          {"type": "text", "text": "请一句话描述这张图片里有什么内容？"},
#          {"type": "image_url", "image_url": {"url": "https://upload.wikimedia.org/wikipedia/commons/thumb/2/25/Dr_Abraham_Verghese_in_2023_06.jpg/500px-Dr_Abraham_Verghese_in_2023_06.jpg", "detail": "auto"}}
#        ]
#      }
#    ],
#    "max_tokens": 500
#  }'
# ============================================================================

set -uo pipefail

# ==================== 配置 ====================
BASE_URL="${KC_BASE_URL:-http://localhost:3000}"
BASE_URL="${BASE_URL%/v1/chat/completions}"
BASE_URL="${BASE_URL%/v1}"
# 始终绕过全局代理（避免 localhost 请求被代理到外部）
CURL_EXTRA_FLAGS=(--noproxy '*')
# 如果 BASE_URL 以 https 开头，额外添加 -k (跳过证书验证，用于自签名证书)
if [[ "$BASE_URL" == https://* ]]; then
    CURL_EXTRA_FLAGS+=(-k)
fi
API_KEY="${KC_API_KEY:-sk-your-api-key-here}"
MODEL="${KC_MODEL:-gemma3:4b}"

# ==================== 参数处理 ====================
IMAGE_URL="${1:-https://upload.wikimedia.org/wikipedia/commons/thumb/2/25/Dr_Abraham_Verghese_in_2023_06.jpg/500px-Dr_Abraham_Verghese_in_2023_06.jpg}"
QUESTION="${2:-请一句话描述这张图片里有什么内容？}"

# ==================== 前置检查 ====================
if ! command -v curl &>/dev/null; then
    printf '错误: 未找到 curl，请先安装 curl\n' >&2
    exit 1
fi

if [ -z "${API_KEY}" ] || [ "${API_KEY}" = "sk-your-api-key-here" ]; then
    printf '警告: API Key 未设置或使用占位值，请求将被拒绝\n' >&2
    printf '  请通过环境变量指定: KC_API_KEY=sk-实际密钥\n' >&2
fi

# ==================== 构建请求体 ====================
REQUEST_BODY=$(cat <<EOF
{
  "model": "${MODEL}",
  "messages": [
    {
      "role": "user",
      "content": [
        {"type": "text", "text": "${QUESTION}"},
        {"type": "image_url", "image_url": {"url": "${IMAGE_URL}", "detail": "auto"}}
      ]
    }
  ],
  "max_tokens": 500
}
EOF
)

# ==================== 执行请求 ====================
printf '📷 KeyCompute 图片理解 API 调用示例\n'
printf '服务端地址: %s\n' "${BASE_URL}"
printf '模型:       %s\n' "${MODEL}"
printf '图片URL:    %s\n' "${IMAGE_URL}"
printf '问题:       %s\n' "${QUESTION}"
printf '\n'
printf '▶ 发送请求...\n\n'

# 保存响应到临时文件以便调试
TMPFILE=$(mktemp)
trap "rm -f $TMPFILE" EXIT

HTTP_CODE=$(curl -sS -w '%{http_code}' -o "$TMPFILE" "${CURL_EXTRA_FLAGS[@]}" -X POST "${BASE_URL}/v1/chat/completions" \
  -H "Authorization: Bearer ${API_KEY}" \
  -H "Content-Type: application/json" \
  -d "$REQUEST_BODY")

printf '\n📥 HTTP 状态码: %s\n' "$HTTP_CODE"
printf '📥 原始响应:\n'
cat "$TMPFILE"
printf '\n\n📥 格式化响应:\n'

if [ "$HTTP_CODE" -ge 200 ] && [ "$HTTP_CODE" -lt 300 ]; then
    if command -v jq &>/dev/null; then
      jq '.' "$TMPFILE" 2>/dev/null || cat "$TMPFILE"
    elif command -v python3 &>/dev/null; then
      python3 -m json.tool "$TMPFILE" 2>/dev/null || cat "$TMPFILE"
    else
      cat "$TMPFILE"
    fi
else
    printf '⚠️  请求失败\n'
    cat "$TMPFILE"
fi

printf '\n✅ 请求完成\n'
