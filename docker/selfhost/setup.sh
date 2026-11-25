#!/usr/bin/env bash
# Create .env from .env.example, filling every empty secret with a generated
# value. Safe to run again: a value that is already set is never replaced, so
# secrets stay stable once the stack has been started.
#
#   ./setup.sh            # create or complete .env
#   ./setup.sh --host macro.example.com --email you@example.com
#                         # also point the stack at a public hostname
set -euo pipefail

cd "$(dirname "$0")"

host=""
email=""
while [ $# -gt 0 ]; do
  case "$1" in
    --host) host="${2:?--host needs a hostname}"; shift 2 ;;
    --email) email="${2:?--email needs an address}"; shift 2 ;;
    -h|--help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

command -v openssl >/dev/null || { echo "openssl is required" >&2; exit 1; }
command -v ssh-keygen >/dev/null || { echo "ssh-keygen (OpenSSH) is required" >&2; exit 1; }

[ -f .env ] || cp .env.example .env

# get NAME: the value currently in .env (empty if unset).
get() { sed -n "s/^$1=//p" .env | head -n1; }

# set NAME VALUE: replace the line, or append one. The value goes through the
# environment, not the command line, so awk does not treat backslashes in a PEM
# key as escapes.
set_var() {
  local tmp
  tmp="$(mktemp)"
  NAME="$1" VALUE="$2" awk '
    BEGIN { name = ENVIRON["NAME"]; value = ENVIRON["VALUE"]; done = 0 }
    index($0, name "=") == 1 { print name "=" value; done = 1; next }
    { print }
    END { if (!done) print name "=" value }
  ' .env > "$tmp"
  cat "$tmp" > .env
  rm -f "$tmp"
}

# fill NAME GENERATOR...: set NAME from the generator unless it already has a value.
fill() {
  local name="$1"; shift
  [ -n "$(get "$name")" ] || set_var "$name" "$("$@")"
}

hex()   { openssl rand -hex "$1"; }
hex48() { hex 24; }
hex32() { hex 32; }
b64url() { openssl rand -base64 32 | tr '+/' '-_' | tr -d '=\n'; }
b64()   { openssl rand -base64 32 | tr -d '\n'; }
uuid() {
  local h; h="$(hex 16)"
  echo "${h:0:8}-${h:8:4}-4${h:13:3}-8${h:17:3}-${h:20:12}"
}
# A PEM on one line, its newlines written as \n inside double quotes: Compose
# expands them when it reads .env.
oneline() { printf '"%s"' "$(awk 'NF { printf "%s\\n", $0 }')"; }
pkcs8_private() { openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 2>/dev/null; }
pkcs1_private() {
  local key
  key="$(openssl genrsa 2048 2>/dev/null)"
  # OpenSSL 3 writes PKCS#8 by default; the services read the PKCS#1 form.
  if ! grep -q 'BEGIN RSA PRIVATE KEY' <<<"$key"; then
    key="$(openssl rsa -traditional <<<"$key" 2>/dev/null)"
  fi
  printf '%s\n' "$key"
}

# The agent harness's preview gateway SSH host identity.
ssh_host_key() {
  local dir
  dir="$(mktemp -d)"
  ssh-keygen -q -t ed25519 -N "" -C macro-selfhost-preview -f "$dir/key"
  cat "$dir/key"
  rm -rf "$dir"
}

fill POSTGRES_PASSWORD            hex48
fill FUSIONAUTH_DB_PASSWORD       hex48
fill FUSIONAUTH_ADMIN_PASSWORD    hex 12
fill INTERNAL_API_SECRET_KEY      hex32
fill DOCUMENT_PERMISSION_SECRET   hex32
fill INTERNAL_CALL_SECRET         hex32
fill URL_SIGNING_HMAC             hex32
fill APP_SECRET                   hex32
fill WEBHOOK_SECRET               hex32
fill JWT_SECRET_KEY               hex32
fill FUSIONAUTH_API_KEY           uuid
fill FUSIONAUTH_CLIENT_SECRET     b64url
fill MCP_CREDENTIALS_KEY          b64

if [ -z "$(get MACRO_API_TOKEN_PRIVATE_KEY)" ] || [ -z "$(get MACRO_API_TOKEN_PUBLIC_KEY)" ]; then
  private_pem="$(pkcs8_private)"
  set_var MACRO_API_TOKEN_PRIVATE_KEY "$(oneline <<<"$private_pem")"
  set_var MACRO_API_TOKEN_PUBLIC_KEY "$(openssl pkey -pubout <<<"$private_pem" | oneline)"
fi
[ -n "$(get PREVIEW_SSH_HOST_KEY)" ] || set_var PREVIEW_SSH_HOST_KEY "$(ssh_host_key | oneline)"
[ -n "$(get ATTACHMENT_LINK_SIGNING_KEY)" ] || set_var ATTACHMENT_LINK_SIGNING_KEY "$(pkcs1_private | oneline)"

if [ -n "$host" ]; then
  set_var SITE_ADDRESS "$host"
  set_var PUBLIC_ORIGIN "https://$host"
  set_var PROXY_PORT 443
  set_var TLS_MODE "${email:-internal}"
fi

chmod 600 .env
echo "Wrote $(pwd)/.env"
echo
echo "Next:  docker compose up -d"
echo "Then open $(get PUBLIC_ORIGIN) and sign in with any email address."
echo "FusionAuth admin (http://localhost:$(get FUSIONAUTH_PORT)): $(get FUSIONAUTH_ADMIN_EMAIL) / $(get FUSIONAUTH_ADMIN_PASSWORD)"
