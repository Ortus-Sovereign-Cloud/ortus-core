#!/usr/bin/env bash
# Starts a throwaway OpenLDAP directory (test only) on <port> with the
# people and groups in tests/fixtures/ldap-seed.ldif, and prints the
# setting to export (ORTUS_TEST_LDAP).
set -euo pipefail
cd "$(dirname "$0")/.."
port="${1:-51389}"
docker rm -f ortus-test-ldap >/dev/null 2>&1 || true
docker run -d --rm --name ortus-test-ldap -p "127.0.0.1:$port:389" -e LDAP_ORGANISATION=Ministry -e LDAP_DOMAIN=ministry.example -e LDAP_ADMIN_PASSWORD=adminpass osixia/openldap:1.5.0 >/dev/null
for _ in $(seq 60); do
  docker exec ortus-test-ldap ldapsearch -x -H ldap://localhost -b dc=ministry,dc=example -D cn=admin,dc=ministry,dc=example -w adminpass >/dev/null 2>&1 && break
  sleep 1
done
docker cp tests/fixtures/ldap-seed.ldif ortus-test-ldap:/tmp/seed.ldif
docker exec ortus-test-ldap ldapadd -x -H ldap://localhost -D cn=admin,dc=ministry,dc=example -w adminpass -f /tmp/seed.ldif >/dev/null
echo "export ORTUS_TEST_LDAP=ldap://127.0.0.1:$port"
