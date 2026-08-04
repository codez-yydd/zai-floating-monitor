#!/usr/bin/env bash
# zbar-sync 启动脚本
# 用法：./start.sh  或  PORT=8080 ./start.sh
#
# 首次运行会自动创建 zbar-data/ 目录、建库、生成 master token。
# 日志会打印 master token，复制到客户端注册设备用。

set -e
cd "$(dirname "$0")"

# 默认端口 3838，可通过环境变量 PORT 覆盖
export PORT="${PORT:-3838}"

echo "启动 zbar-sync（端口 $PORT）..."
python3 app.py
