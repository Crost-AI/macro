#!/usr/bin/env bash
# Offline regression test for the deploy/.env -> xtask self-host contract.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_EXAMPLE="${REPO_ROOT}/deploy/.env.example"
UP_SCRIPT="${REPO_ROOT}/deploy/selfhost-up.sh"

fail() {
  printf 'selfhost config test: %s\n' "$*" >&2
  exit 1
}

if grep -Eq '^[[:space:]]*MACRO_DB_URL=.*(localhost|127\.0\.0\.1)' "$ENV_EXAMPLE"; then
  fail 'deploy/.env.example must not inject a host-loopback MACRO_DB_URL into containers'
fi

grep -Fq 'FUSIONAUTH_OAUTH_REDIRECT_URI=http://100.64.0.20:31011/oauth/redirect' "$ENV_EXAMPLE" \
  || fail 'private-network OAuth redirect example is missing'
grep -Fq 'WEBHOOK_URL=http://100.64.0.20:8080/webhooks/macro' "$ENV_EXAMPLE" \
  || fail 'Crost webhook example is missing'

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
log_file="${tmp_dir}/commands.log"
env_file="${tmp_dir}/selfhost.env"
stub_bin="${tmp_dir}/bin"
mkdir -p "$stub_bin"

cat >"$env_file" <<'EOF'
MACRO_INSTANCE=config-test
MACRO_PORT_BASE=32000
MACRO_STACK_PROJECT=macro-config-test
MACRO_SELFHOST_SEED=false
MACRO_SELFHOST_KEEPALIVE=
WEBHOOK_URL=http://100.64.0.20:8080/webhooks/macro
WEBHOOK_SECRET=test-secret
EOF

cat >"${stub_bin}/just" <<'EOF'
#!/usr/bin/env bash
printf 'just' >>"$SELFHOST_TEST_LOG"
printf ' <%s>' "$@" >>"$SELFHOST_TEST_LOG"
printf '\n' >>"$SELFHOST_TEST_LOG"
EOF
chmod +x "${stub_bin}/just"

cat >"${stub_bin}/bun" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "${stub_bin}/bun"

if ! PATH="${stub_bin}:/usr/bin:/bin" \
  SELFHOST_TEST_LOG="$log_file" \
  MACRO_REPO_ROOT="$REPO_ROOT" \
  MACRO_ENV_FILE="$env_file" \
  bash "$UP_SCRIPT" >"${tmp_dir}/stdout.log" 2>"${tmp_dir}/stderr.log"; then
  fail "selfhost wrapper failed: $(tr '\\n' ';' <"${tmp_dir}/stderr.log")"
fi

expected="just <stack> <up> <--no-doppler> <--instance> <config-test> <--port-base> <32000> <--env-file> <${env_file}>"
grep -Fqx "$expected" "$log_file" \
  || fail "stack up did not receive the configured env file (commands: $(tr '\n' ';' <"$log_file"))"

printf 'selfhost config test: ok\n'
