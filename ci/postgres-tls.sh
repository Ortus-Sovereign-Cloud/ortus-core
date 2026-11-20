#!/usr/bin/env bash
# Starts a throwaway PostgreSQL that speaks TLS with its own CA, for the
# TLS tests: ci/postgres-tls.sh <port> <dir>. Prints the settings to
# export (ORTUS_TEST_POSTGRES_TLS and ORTUS_TEST_POSTGRES_TLS_CA).
set -euo pipefail
port="${1:-55433}"
dir="${2:-$(mktemp -d)}"
mkdir -p "$dir"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=Ortus test CA" -keyout "$dir/ca.key" -out "$dir/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout "$dir/server.key" -out "$dir/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\n' > "$dir/san.ext"
openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.pem" -CAkey "$dir/ca.key" -CAcreateserial -days 2 -extfile "$dir/san.ext" -out "$dir/server.pem" 2>/dev/null
chmod 0644 "$dir/server.pem" "$dir/ca.pem"
chmod 0600 "$dir/server.key"
docker rm -f ortus-test-pg-tls >/dev/null 2>&1 || true
docker run -d --rm --name ortus-test-pg-tls -e POSTGRES_PASSWORD=ortus -p "127.0.0.1:$port:5432" \
  -v "$dir:/certs:ro" --entrypoint sh postgres:17-alpine -c \
  'cp /certs/server.pem /certs/server.key /tmp/ && chown postgres /tmp/server.* && chmod 600 /tmp/server.key && exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/server.pem -c ssl_key_file=/tmp/server.key' >/dev/null
for _ in $(seq 60); do
  docker exec ortus-test-pg-tls pg_isready -U postgres >/dev/null 2>&1 && break
  sleep 1
done
sleep 2
echo "export ORTUS_TEST_POSTGRES_TLS=postgres://postgres:ortus@localhost:$port/postgres ORTUS_TEST_POSTGRES_TLS_CA=$dir/ca.pem"
