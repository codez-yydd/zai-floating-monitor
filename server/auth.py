"""鉴权：Master Token（注册/清理用）与 Device Token（日常同步用）。

两层设计（与 Rust 版一致）：
- Master Token：服务启动自动生成，只用于注册新设备和清理数据。
- Device Token：注册时为每台设备生成，日常同步/查询都用它。服务端只存哈希。
"""

import hashlib
import secrets

from config import MASTER_TOKEN_PATH


def hash_token(token):
    """device_token 的 SHA-256 hex（64 字符），存入 devices 表。"""
    return hashlib.sha256(token.encode()).hexdigest()


def random_hex():
    """生成 32 字节随机 hex 字符串（64 字符）。用于 master_token / device_token。"""
    return secrets.token_hex(32)


def random_device_id():
    """生成 uuid 风格的 device_id（8-4-4-4-12 格式）。"""
    h = random_hex()  # 64 字符，取前 32 用
    return f"{h[0:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:32]}"


def load_or_create_master_token():
    """读取或生成 master token。
    文件存在则读取；不存在则生成并写入。启动时调用，并打印到日志供用户复制。
    """
    if MASTER_TOKEN_PATH.exists():
        tok = MASTER_TOKEN_PATH.read_text().strip()
        if tok:
            return tok
    # 生成新的
    MASTER_TOKEN_PATH.parent.mkdir(parents=True, exist_ok=True)
    tok = random_hex()
    MASTER_TOKEN_PATH.write_text(tok)
    return tok


def safe_eq(a, b):
    """常量时间比较两个字符串，避免计时攻击。"""
    return secrets.compare_digest(a, b)
