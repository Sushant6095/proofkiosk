# ProofKiosk demo shell setup.  SOURCE this, do not execute it:
#
#     source scripts/demo-env.sh
#
# Every shell needs it again — it defines a function and edits PATH, neither of
# which survives closing the terminal. It exists so the runbook's environment
# block never has to be copy-pasted out of a PDF, where long `$FOO_BAR` names
# split across lines and produce `command not found: BAR`.

# Guard against being run instead of sourced: a subshell would exit and take
# every export with it, leaving the caller looking at the same errors.
case "${ZSH_EVAL_CONTEXT:-}${BASH_SOURCE[0]:+file}" in
  *toplevel*|*file*) : ;;
  *) printf 'run this as:  source %s\n' "$0" >&2; exit 1 ;;
esac

PROOFKIOSK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-${(%):-%x}}")/.." && /bin/pwd -P)"
export PROOFKIOSK_ROOT

export ZEROCLAW_SRC="$PROOFKIOSK_ROOT/.build/zeroclaw-e112ce6b5ccd"
export ZC_CONFIG_DIR="$PROOFKIOSK_ROOT/.devnet/zeroclaw-config"
export ZC_AGENT="proofkiosk"
export ITEM_ID="cold_drink"

mkdir -p "$PROOFKIOSK_ROOT/.devnet" "$ZC_CONFIG_DIR"

# The pinned plugin-capable host must come BEFORE any cargo-installed zeroclaw.
# The stock 0.8.3 binary has no WASM plugin runtime, so silently keeping it on
# PATH produces "unrecognized subcommand 'plugin'" much later and further away.
PINNED_BIN="$PROOFKIOSK_ROOT/.build/zeroclaw-install/bin"
if [ ! -x "$PINNED_BIN/zeroclaw" ]; then
  printf '\033[1;31m[demo-env] pinned host missing.\033[0m Run first:\n' >&2
  printf '  ./scripts/install-pinned-zeroclaw.sh\n' >&2
else
  case ":$PATH:" in
    *":$PINNED_BIN:"*) : ;;
    *) PATH="$PINNED_BIN:$PATH"; export PATH ;;
  esac
  hash -r 2>/dev/null || true
fi

# Routes every ZeroClaw command at the isolated, gitignored test config so the
# demo never touches ~/.zeroclaw.
zc() { zeroclaw --config-dir "$ZC_CONFIG_DIR" "$@"; }

# If a previous run of scripts/devnet-setup.sh left a handoff, load it too —
# that is where MERCHANT/MINT/REFERENCE and friends come from.
if [ -f "$PROOFKIOSK_ROOT/.devnet/payment.env" ]; then
  # shellcheck disable=SC1091
  . "$PROOFKIOSK_ROOT/.devnet/payment.env"
  printf '[demo-env] loaded .devnet/payment.env\n'
fi

printf '\033[1;32m[demo-env] ready\033[0m  zeroclaw=%s  agent=%s\n' \
  "$(zeroclaw --version 2>/dev/null | awk '{print $2}')" "$ZC_AGENT"
printf '[demo-env] config-dir=%s\n' "$ZC_CONFIG_DIR"
