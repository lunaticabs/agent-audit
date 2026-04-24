#!/usr/bin/env bash
# =============================================================================
# MongoDB TLS 证书生成脚本（Ed25519）
# 生成内容：CA、服务端证书、客户端证书（可选 mTLS）
# 用法：bash gen-mongodb-certs.sh [输出目录] [服务器IP或域名...]
# 示例：bash gen-mongodb-certs.sh ./ssl 127.0.0.1 192.168.1.10 mongo.example.com
# =============================================================================

set -euo pipefail

# ──────────────────────────────────────────────
# 参数处理
# ──────────────────────────────────────────────
OUTPUT_DIR="${1:-./ssl}"
shift || true
EXTRA_SANS=("$@")  # 额外的 IP 或 DNS

# ──────────────────────────────────────────────
# 配置项（按需修改）
# ──────────────────────────────────────────────
DAYS_CA=3650          # CA 有效期（天）
DAYS_CERT=825         # 服务端/客户端证书有效期（天）
COUNTRY="JP"
STATE="Osaka"
ORG="MyOrg"
CA_CN="MyMongoDB-CA"
SERVER_CN="mongodb-server"
CLIENT_CN="mongodb-client"

# ──────────────────────────────────────────────
# 颜色输出
# ──────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()    { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; exit 1; }

# ──────────────────────────────────────────────
# 依赖检查
# ──────────────────────────────────────────────
command -v openssl &>/dev/null || error "未找到 openssl，请先安装。"

OPENSSL_VER=$(openssl version | awk '{print $2}')
info "OpenSSL 版本：$OPENSSL_VER"

# Ed25519 需要 OpenSSL 1.1.1+
MIN_VER="1.1.1"
if [[ "$(printf '%s\n' "$MIN_VER" "$OPENSSL_VER" | sort -V | head -n1)" != "$MIN_VER" ]]; then
    error "OpenSSL 版本过低（需要 >= 1.1.1），当前：$OPENSSL_VER"
fi

# ──────────────────────────────────────────────
# 准备输出目录
# ──────────────────────────────────────────────
mkdir -p "$OUTPUT_DIR"
cd "$OUTPUT_DIR"
info "证书输出目录：$(pwd)"

# ──────────────────────────────────────────────
# 构建 subjectAltName
# ──────────────────────────────────────────────
SAN_LIST="IP:144.76.234.107"
for san in "${EXTRA_SANS[@]+"${EXTRA_SANS[@]}"}"; do
    if [[ "$san" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        SAN_LIST+=",IP:$san"
    else
        SAN_LIST+=",DNS:$san"
    fi
done
info "subjectAltName：$SAN_LIST"

# ──────────────────────────────────────────────
# 1. 生成 CA
# ──────────────────────────────────────────────
info "─── 生成 CA 私钥和证书..."
openssl genpkey -algorithm ED25519 -out ca.key

openssl req -new -x509 \
    -key ca.key \
    -out ca.crt \
    -days "$DAYS_CA" \
    -subj "/C=${COUNTRY}/ST=${STATE}/O=${ORG}/CN=${CA_CN}"

# ──────────────────────────────────────────────
# 2. 生成服务端证书
# ──────────────────────────────────────────────
info "─── 生成服务端私钥和证书..."
openssl genpkey -algorithm ED25519 -out server.key

openssl req -new \
    -key server.key \
    -out server.csr \
    -subj "/C=${COUNTRY}/ST=${STATE}/O=${ORG}/CN=${SERVER_CN}"

openssl x509 -req \
    -in server.csr \
    -CA ca.crt \
    -CAkey ca.key \
    -CAcreateserial \
    -out server.crt \
    -days "$DAYS_CERT" \
    -extfile <(printf "subjectAltName=%s\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment" "$SAN_LIST")

# MongoDB 要求私钥和证书合并为单一 .pem 文件
cat server.key server.crt > server.pem

# ──────────────────────────────────────────────
# 3. 生成客户端证书（用于 mTLS）
# ──────────────────────────────────────────────
info "─── 生成客户端私钥和证书..."
openssl genpkey -algorithm ED25519 -out client.key

openssl req -new \
    -key client.key \
    -out client.csr \
    -subj "/C=${COUNTRY}/ST=${STATE}/O=${ORG}/CN=${CLIENT_CN}"

openssl x509 -req \
    -in client.csr \
    -CA ca.crt \
    -CAkey ca.key \
    -CAcreateserial \
    -out client.crt \
    -days "$DAYS_CERT" \
    -extfile <(printf "basicConstraints=CA:FALSE\nkeyUsage=digitalSignature")

cat client.key client.crt > client.pem

# ──────────────────────────────────────────────
# 4. 设置文件权限
# ──────────────────────────────────────────────
info "─── 设置文件权限..."
chmod 600 ca.key server.key server.pem client.key client.pem
chmod 644 ca.crt server.crt client.crt ca.srl

# ──────────────────────────────────────────────
# 5. 清理临时 CSR 文件
# ──────────────────────────────────────────────
rm -f server.csr client.csr

# ──────────────────────────────────────────────
# 6. 验证证书链
# ──────────────────────────────────────────────
info "─── 验证证书链..."
openssl verify -CAfile ca.crt server.crt \
    && info "服务端证书验证：✅ OK" \
    || warn "服务端证书验证失败"

openssl verify -CAfile ca.crt client.crt \
    && info "客户端证书验证：✅ OK" \
    || warn "客户端证书验证失败"

# ──────────────────────────────────────────────
# 7. 输出文件清单和 compose 配置提示
# ──────────────────────────────────────────────
echo ""
echo -e "${GREEN}══════════════════════════════════════════${NC}"
echo -e "${GREEN}  证书生成完毕${NC}"
echo -e "${GREEN}══════════════════════════════════════════${NC}"
echo ""
echo "📁 输出目录：$(pwd)"
echo ""
echo "  文件                用途"
echo "  ──────────────────────────────────────"
echo "  ca.key              CA 私钥（严格保密，勿外传）"
echo "  ca.crt              CA 证书（分发给所有客户端信任）"
echo "  server.pem          MongoDB 服务端使用（key + crt 合并）"
echo "  client.pem          mTLS 客户端连接使用（key + crt 合并）"
echo "  client.crt          客户端公钥证书"
echo ""
echo "📋 docker-compose.yml 参考配置："
echo ""
cat <<COMPOSE
services:
  mongo:
    image: mongo:8.0
    command: >
      mongod
      --tlsMode requireTLS
      --tlsCertificateKeyFile /etc/ssl/mongodb/server.pem
      --tlsCAFile /etc/ssl/mongodb/ca.pem
    volumes:
      - $(pwd):/etc/ssl/mongodb:ro
      - mongo_data:/data/db
    ports:
      - "27017:27017"

volumes:
  mongo_data:
COMPOSE

echo ""
echo "🔌 连接示例（mongosh）："
echo ""
echo "  mongosh --tls \\"
echo "    --tlsCertificateKeyFile $(pwd)/client.pem \\"
echo "    --tlsCAFile $(pwd)/ca.crt \\"
echo "    'mongodb://admin:secret@127.0.0.1:27017'"
echo ""

# 打印证书有效期
info "证书有效期信息："
echo "  CA         到期：$(openssl x509 -noout -enddate -in ca.crt | cut -d= -f2)"
echo "  server.crt 到期：$(openssl x509 -noout -enddate -in server.crt | cut -d= -f2)"
echo "  client.crt 到期：$(openssl x509 -noout -enddate -in client.crt | cut -d= -f2)"
echo ""


[INFO]  OpenSSL 版本：1.1.1f
[INFO]  证书输出目录：/root/DB/ssl
[INFO]  subjectAltName：IP:127.0.0.1,DNS:localhost
[INFO]  ─── 生成 CA 私钥和证书...
[INFO]  ─── 生成服务端私钥和证书...
Signature ok
subject=C = JP, ST = Osaka, O = MyOrg, CN = mongodb-server
Getting CA Private Key
[INFO]  ─── 生成客户端私钥和证书...
Signature ok
subject=C = JP, ST = Osaka, O = MyOrg, CN = mongodb-client
Getting CA Private Key
[INFO]  ─── 设置文件权限...
[INFO]  ─── 验证证书链...
server.crt: OK
[INFO]  服务端证书验证：✅ OK
client.crt: OK
[INFO]  客户端证书验证：✅ OK

══════════════════════════════════════════
  证书生成完毕
══════════════════════════════════════════

📁 输出目录：/root/DB/ssl

  文件                用途
  ──────────────────────────────────────
  ca.key              CA 私钥（严格保密，勿外传）
  ca.crt              CA 证书（分发给所有客户端信任）
  server.pem          MongoDB 服务端使用（key + crt 合并）
  client.pem          mTLS 客户端连接使用（key + crt 合并）
  client.crt          客户端公钥证书

📋 docker-compose.yml 参考配置：

services:
  mongo:
    image: mongo:8.0
    command: >
      mongod
      --tlsMode requireTLS
      --tlsCertificateKeyFile /etc/ssl/mongodb/server.pem
      --tlsCAFile /etc/ssl/mongodb/ca.crt
    volumes:
      - /root/DB/ssl:/etc/ssl/mongodb:ro
      - mongo_data:/data/db
    ports:
      - "27017:27017"

volumes:
  mongo_data:

🔌 连接示例（mongosh）：

  mongosh --tls \
    --tlsCertificateKeyFile /root/DB/ssl/client.pem \
    --tlsCAFile /root/DB/ssl/ca.crt \
    'mongodb://admin:secret@127.0.0.1:27017'

[INFO]  证书有效期信息：
  CA         到期：Apr 19 07:32:56 2036 GMT
  server.crt 到期：Jul 25 07:32:56 2028 GMT
  client.crt 到期：Jul 25 07:32:56 2028 GMT
  
  
mongodb://admin:secret@127.0.0.1:27017/?tls=true&tlsCAFile=/root/DB/ssl/ca.crt