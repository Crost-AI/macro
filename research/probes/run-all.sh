# Run all W0.1 gate probes (requires crost-trial stack at port-base 31000)

set -euo pipefail
cd "$(dirname "$0")/../.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
export INSTANCE=crost-trial PORT_BASE=31000
export PROXY=http://localhost:31009 STORAGE=http://localhost:31015
mkdir -p research/probes/out
./research/probes/gate3-selfhost.sh | tee research/probes/out/gate3.log
./research/probes/gate1-github-issues.sh | tee research/probes/out/gate1.log
./research/probes/gate4-channels.sh | tee research/probes/out/gate4.log
./research/probes/gate2-webhooks.sh | tee research/probes/out/gate2.log
echo "All probes finished. See research/macro-trial.md"
