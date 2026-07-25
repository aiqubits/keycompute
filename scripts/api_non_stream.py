#!/usr/bin/env python3
"""
简单的对话 Demo，使用 OpenAI 兼容格式的 API（非流式响应）
"""

import readline   # Unix/macOS 自带; Windows 需安装 pyreadline3
from openai import OpenAI

# 配置

API_URL="http://192.168.100.100:3000/v1"
API_KEY="sk-xxxxxxxxxx"
API_MODEL="deepseek-chat"

client = OpenAI(base_url=API_URL, api_key=API_KEY)


def chat():
    messages = []
    print("对话已开始，输入 'quit' 退出\n")

    while True:
        try:
            user_input = input("你: ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\n对话已结束")
            break

        if user_input.lower() == "quit":
            break
        if not user_input:
            continue

        messages.append({"role": "user", "content": user_input})

        try:
            # 非流式响应 (stream=False)
            response = client.chat.completions.create(
                model=API_MODEL,
                messages=messages,
                stream=False,
            )

            print("助手: ", end="", flush=True)
            # 非流式响应直接访问 message.content
            reply = response.choices[0].message.content or ""
            print(reply)
            print()

            messages.append({"role": "assistant", "content": reply})
        except KeyboardInterrupt:
            print("\n对话已结束")
            break
        except Exception as e:
            print(f"\n错误: {e}\n")
            # 移除失败的消息，允许重试
            messages.pop()


if __name__ == "__main__":
    try:
        chat()
    except KeyboardInterrupt:
        print("\n对话已结束")
