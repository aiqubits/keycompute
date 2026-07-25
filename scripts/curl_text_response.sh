#!/usr/bin/env bash
# ============================================================================
# KeyCompute API 调用示例 - curl
# 本脚本演示如何使用 curl 调用 KeyCompute 的 OpenAI 兼容 API
#
# 前置条件: curl (必须), jq 或 python3 (可选, 用于格式化 JSON 输出)
# 用法:
#   chmod +x curl_response.sh
#   ./curl_response.sh                        # 运行所有示例
#   ./curl_response.sh non-stream             # 仅运行指定示例
#   ./curl_response.sh stream node error      # 运行多个示例
#
# 可用示例: non-stream, stream, node-non-stream, node-stream, error
#
# 自定义参数 (环境变量):
#   KC_BASE_URL     服务端地址          (默认: http://localhost:3000)
#   KC_API_KEY      API Key            (默认: sk-your-api-key-here)
#   KC_MODEL        Provider 路径模型  (默认: deepseek-chat)
#   KC_NODE_MODEL   Node 路径模型      (默认: node:gemma3:270m)
#
# 示例:
#   KC_BASE_URL=https://api.example.com KC_API_KEY=sk-xxx ./curl_response.sh
#   KC_MODEL=gemma3:270m ./curl_response.sh non-stream
#   KC_NODE_MODEL=node:llama3 ./curl_response.sh node-stream
# ============================================================================
# curl -X POST "https://192.168.100.100:3000/v1/chat/completions" \
#   -H "Content-Type: application/json" \
#   -H "Authorization: Bearer sk-9273dd79b15b4f258d08a631e23e74d0e97553fdd533439e" \
#   -d '{
#     "model": "gemma3:270m",
#     "messages": [
#       {"role": "system", "content": "你是一个有帮助的助手。"},
#       {"role": "user", "content": "你好，请介绍一下你自己。"}
#     ],
#     "temperature": 0.7,
#     "max_tokens": 512,
#     "stream": false
#   }'
# ============================================================================

# 不全局 set -e，允许单个示例失败后继续执行后续示例
set -uo pipefail

# ==================== 前置检查 ====================
if ! command -v curl &>/dev/null; then
    printf '错误: 未找到 curl，请先安装 curl\n' >&2
    printf '  Ubuntu/Debian: sudo apt install curl\n' >&2
    printf '  macOS:         brew install curl\n' >&2
    exit 1
fi

# ==================== 配置 ====================
# 修改为你的 KeyCompute 服务端地址
BASE_URL="${KC_BASE_URL:-http://localhost:3000}"
# 剥离用户可能误传入的尾部路径（允许 KC_BASE_URL 带 /v1 或 /v1/chat/completions）
BASE_URL="${BASE_URL%/v1/chat/completions}"
BASE_URL="${BASE_URL%/v1}"
# 你的 API Key
API_KEY="${KC_API_KEY:-sk-your-api-key-here}"
# Provider 路径模型名称（非流式/流式示例使用）
DEFAULT_MODEL="${KC_MODEL:-deepseek-chat}"
# Node 路径模型名称（node-non-stream/node-stream 示例使用）
DEFAULT_NODE_MODEL="${KC_NODE_MODEL:-node:gemma3:270m}"

# ==================== 配置校验 ====================
if [ -z "${BASE_URL}" ]; then
    printf '错误: KC_BASE_URL 设置为空，无法连接\n' >&2
    exit 1
fi

if [ -z "${API_KEY}" ] || [ "${API_KEY}" = "sk-your-api-key-here" ]; then
    printf '警告: API Key 未设置或使用占位值，请求将被拒绝\n' >&2
    printf '  请通过环境变量指定: KC_API_KEY=sk-实际密钥\n' >&2
fi

# ==================== 公共参数 ====================
REQUEST_BODY_COMMON='{
  "messages": [
    {"role": "system", "content": "你是一个有帮助的助手。"},
    {"role": "user", "content": "你好，请介绍一下你自己。"}
  ],
  "temperature": 0.7,
  "max_tokens": 512
}'

COLOR_RESET='\033[0m'
COLOR_GREEN='\033[0;32m'
COLOR_CYAN='\033[0;36m'
COLOR_YELLOW='\033[1;33m'

print_usage() {
    printf '%b用法:%b\n' "${COLOR_GREEN}" "${COLOR_RESET}"
    printf '  %b./curl_response.sh%b [示例名称...]\n' "${COLOR_CYAN}" "${COLOR_RESET}"
    printf '  %b./curl_response.sh --help%b\n' "${COLOR_CYAN}" "${COLOR_RESET}"
    printf '\n'
    printf '%b可用示例:%b\n' "${COLOR_GREEN}" "${COLOR_RESET}"
    printf '  non-stream       非流式请求 → Provider 路径\n'
    printf '  stream           流式请求   → Provider 路径\n'
    printf '  node-non-stream  非流式请求 → Node 路径\n'
    printf '  node-stream      流式请求   → Node 路径（服务端模拟流式）\n'
    printf '  error            错误场景   → Node 路径无可用节点\n'
    printf '\n'
    printf '%b自定义参数 (环境变量):%b\n' "${COLOR_GREEN}" "${COLOR_RESET}"
    printf '  KC_BASE_URL      服务端地址          (默认: http://localhost:3000)\n'
    printf '  KC_API_KEY       API Key            (默认: sk-your-api-key-here)\n'
    printf '  KC_MODEL         Provider 路径模型  (默认: deepseek-chat)\n'
    printf '  KC_NODE_MODEL    Node 路径模型      (默认: node:gemma3:270m)\n'
    printf '\n'
    printf '  %s# 完整示例:%s\n' "${COLOR_YELLOW}" "${COLOR_RESET}"
    printf '  KC_BASE_URL=http://localhost:3000 '
    printf 'KC_API_KEY=sk-xxx '
    printf 'KC_MODEL=deepseek-chat '
    printf './curl_response.sh\n'
}

print_header() {
    printf '\n%b═══════════════════════════════════════════════════════════════%b\n' "${COLOR_CYAN}" "${COLOR_RESET}"
    printf '%b▶ %s%b\n' "${COLOR_GREEN}" "$1" "${COLOR_RESET}"
    printf '%b═══════════════════════════════════════════════════════════════%b\n\n' "${COLOR_CYAN}" "${COLOR_RESET}"
}

print_json() {
    if command -v jq &>/dev/null; then
        jq . <<<"$1" 2>/dev/null || printf '%s\n' "$1"
    elif command -v python3 &>/dev/null; then
        python3 -m json.tool <<<"$1" 2>/dev/null || printf '%s\n' "$1"
    elif command -v python &>/dev/null; then
        python -m json.tool <<<"$1" 2>/dev/null || printf '%s\n' "$1"
    else
        printf '%s\n' "$1"
    fi
}

# 安全执行 curl，检查 HTTP 状态码
# 参数: 所有参数透传给 curl
# 返回: curl 的退出码
curl_with_check() {
    local http_code
    local tmpfile
    tmpfile=$(mktemp)
    trap "rm -f '${tmpfile}'" RETURN

    # 非流式请求：设 30s 连接超时 + 180s 总超时
    # --noproxy '*' 绕过全局代理，避免 localhost 请求被转发到外部
    http_code=$(curl -sS --noproxy '*' --connect-timeout 30 --max-time 180 \
        -w '%{http_code}' -o "$tmpfile" "$@") || {
        local rc=$?
        printf '    ⚠ curl 请求失败 (退出码 %d)\n' "$rc" >&2
        return "$rc"
    }

    if [ "$http_code" -ge 400 ]; then
        printf '    ⚠ HTTP %s (错误响应):\n' "$http_code" >&2
        if [ -s "$tmpfile" ]; then
            print_json "$(<"$tmpfile")" >&2
        fi
        return 1
    fi

    if [ -s "$tmpfile" ]; then
        print_json "$(<"$tmpfile")"
    fi
}

# 构建 JSON 请求体。优先使用 jq，fallback 到 python3/python，最后手动拼接
build_body() {
    local model="$1"
    local stream="$2"

    if command -v jq &>/dev/null; then
        printf '%s\n' "$REQUEST_BODY_COMMON" | jq --arg model "$model" --argjson stream "$stream" \
            '. + {model: $model, stream: $stream}'
    elif command -v python3 &>/dev/null; then
        python3 -c "
import json
import sys

body = json.loads(sys.argv[1])
body['model'] = sys.argv[2]
body['stream'] = sys.argv[3].lower() == 'true'
print(json.dumps(body))
" "$REQUEST_BODY_COMMON" "$model" "$stream"
    elif command -v python &>/dev/null; then
        python -c "
import json
import sys

body = json.loads(sys.argv[1])
body['model'] = sys.argv[2]
body['stream'] = sys.argv[3].lower() == 'true'
print(json.dumps(body))
" "$REQUEST_BODY_COMMON" "$model" "$stream"
    else
        # 最后的 fallback：手动拼接 JSON
        # 警告：消息体内容必须与 $REQUEST_BODY_COMMON 保持同步
        printf '{"model":"%s","messages":[{"role":"system","content":"\u4f60\u662f\u4e00\u4e2a\u6709\u5e2e\u52a9\u7684\u52a9\u624b\u3002"},{"role":"user","content":"\u4f60\u597d\uff0c\u8bf7\u4ecb\u7ecd\u4e00\u4e0b\u4f60\u81ea\u5df1\u3002"}],"temperature":0.7,"max_tokens":512,"stream":%s}' \
            "$model" "$stream"
    fi
}

# ==================== 示例 1: 非流式请求 (Provider 路径) ====================
example_non_stream() {
    print_header "示例 1: 非流式请求 → Provider 路径 (model=${DEFAULT_MODEL})"

    local model="${1:-${DEFAULT_MODEL}}"
    local body
    body=$(build_body "$model" false)

    printf '%b请求:%b\n' "${COLOR_YELLOW}" "${COLOR_RESET}"
    printf '    POST %s/v1/chat/completions\n' "${BASE_URL}"
    printf '    model: %s (stream=false)\n' "$model"
    printf '\n'

    curl_with_check -X POST "${BASE_URL}/v1/chat/completions" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${API_KEY}" \
        -d "$body"

    printf '\n'
}

# ==================== 示例 2: 流式请求 (Provider 路径) ====================
example_stream() {
    print_header "示例 2: 流式请求 → Provider 路径 (model=${DEFAULT_MODEL})"

    local model="${1:-${DEFAULT_MODEL}}"
    local body
    body=$(build_body "$model" true)

    printf '%b请求:%b\n' "${COLOR_YELLOW}" "${COLOR_RESET}"
    printf '    POST %s/v1/chat/completions\n' "${BASE_URL}"
    printf '    model: %s (stream=true)\n' "$model"
    printf '\n'

    # 流式响应: 使用 -N (--no-buffer) 实时显示 SSE 事件
    # --noproxy '*' 绕过全局代理，避免 localhost 请求被转发到外部
    curl -sS --noproxy '*' -N -X POST "${BASE_URL}/v1/chat/completions" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${API_KEY}" \
        -d "$body" || {
        local rc=$?
        printf '    ⚠ curl 流式请求失败 (退出码 %d)\n' "$rc" >&2
    }

    # 流式输出末尾追加换行（若响应未以换行结尾）
    printf '\n'
}

# ==================== 示例 3: 非流式请求 (Node 路径) ====================
example_node_non_stream() {
    print_header "示例 3: 非流式请求 → Node 路径 (model=${DEFAULT_NODE_MODEL})"

    local model="${1:-${DEFAULT_NODE_MODEL}}"
    local body
    body=$(build_body "$model" false)

    printf '%b请求:%b\n' "${COLOR_YELLOW}" "${COLOR_RESET}"
    printf '    POST %s/v1/chat/completions\n' "${BASE_URL}"
    printf '    model: %s (stream=false, node: 前缀命中 Node 执行路径)\n' "$model"
    printf '\n'

    curl_with_check -X POST "${BASE_URL}/v1/chat/completions" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${API_KEY}" \
        -d "$body"

    printf '\n'
}

# ==================== 示例 4: 流式请求 (Node 路径) ====================
# 注意: node-token 永远使用非流式调用 Ollama,
#       但服务端会在收到完整响应后模拟流式输出
example_node_stream() {
    print_header "示例 4: 流式请求 → Node 路径 (model=${DEFAULT_NODE_MODEL}, 服务端模拟流式)"

    local model="${1:-${DEFAULT_NODE_MODEL}}"
    local body
    body=$(build_body "$model" true)

    printf '%b请求:%b\n' "${COLOR_YELLOW}" "${COLOR_RESET}"
    printf '    POST %s/v1/chat/completions\n' "${BASE_URL}"
    printf '    model: %s (stream=true, node: 前缀 → 服务端模拟流式输出)\n' "$model"
    printf '\n'

    # --noproxy '*' 绕过全局代理，避免 localhost 请求被转发到外部
    curl -sS --noproxy '*' -N -X POST "${BASE_URL}/v1/chat/completions" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${API_KEY}" \
        -d "$body" || {
        local rc=$?
        printf '    ⚠ curl 流式请求失败 (退出码 %d)\n' "$rc" >&2
    }

    printf '\n'
}

# ==================== 示例 5: 错误场景 - Node 路径无可用节点 ====================
example_error() {
    print_header "示例 5: 错误场景 → Node 路径 (无可用节点)"

    local model="node:non-existent-model"
    local body
    body=$(build_body "$model" false)

    printf '%b请求:%b\n' "${COLOR_YELLOW}" "${COLOR_RESET}"
    printf '    POST %s/v1/chat/completions\n' "${BASE_URL}"
    printf '    model: %s (无可用节点, 预期返回错误)\n' "$model"
    printf '\n'

    curl_with_check -X POST "${BASE_URL}/v1/chat/completions" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${API_KEY}" \
        -d "$body" || true  # 错误场景允许失败

    printf '\n'
}

# ==================== 主入口 ====================
main() {
    # 对 API Key 做脱敏显示
    local masked_key
    if [ "${#API_KEY}" -le 8 ]; then
        masked_key='****'
    else
        local prefix="${API_KEY:0:3}"
        local suffix="${API_KEY: -4}"
        masked_key="${prefix}...${suffix}"
    fi

    printf '%bKeyCompute API 调用示例%b\n' "${COLOR_CYAN}" "${COLOR_RESET}"
    printf '服务端地址: %s\n' "${BASE_URL}"
    printf 'API Key:    %s\n' "$masked_key"
    printf 'Provider模型: %s\n' "${DEFAULT_MODEL}"
    printf 'Node模型:    %s\n' "${DEFAULT_NODE_MODEL}"
    printf '\n'

    if [ "$#" -eq 0 ]; then
        # 无参数: 运行所有示例
        example_non_stream
        example_stream
        example_node_non_stream
        example_node_stream
        example_error
    else
        for arg in "$@"; do
            case "$arg" in
                non-stream|non_stream)
                    example_non_stream ;;
                stream)
                    example_stream ;;
                node-non-stream|node_non_stream)
                    example_node_non_stream ;;
                node-stream|node_stream)
                    example_node_stream ;;
                error)
                    example_error ;;
                help|--help|-h)
                    print_usage
                    exit 0 ;;
                *)
                    printf '%b未知示例: %s%b\n' "${COLOR_YELLOW}" "$arg" "${COLOR_RESET}"
                    printf '请使用 %b./curl_response.sh --help%b 查看完整用法\n' "${COLOR_CYAN}" "${COLOR_RESET}"
                    ;;
            esac
        done
    fi

    printf '\n%b✅ 所有请求完成%b\n' "${COLOR_GREEN}" "${COLOR_RESET}"
}

# ==================== 入口 ====================
# 仅在直接执行时运行（非 source 导入）
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
