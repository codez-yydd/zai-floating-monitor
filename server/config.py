"""配置：从环境变量读取，提供默认值。

部署时可通过环境变量覆盖：
  PORT      监听端口（默认 3838）
  DATA_DIR  数据目录（默认 ./zbar-data，库和 master.token 都在这）
  HOST      监听地址（默认 0.0.0.0，对外可访问）
"""

import os
from pathlib import Path

# 监听端口
PORT = int(os.environ.get("PORT", "3838"))

# 监听地址：0.0.0.0 = 对所有网络接口开放（局域网/公网都能访问）
HOST = os.environ.get("HOST", "0.0.0.0")

# 数据目录：usage.db 和 master.token 都存在这里
DATA_DIR = Path(os.environ.get("DATA_DIR", "./zbar-data"))

# 数据库文件路径
DB_PATH = DATA_DIR / "usage.db"

# master token 文件路径
MASTER_TOKEN_PATH = DATA_DIR / "master.token"
