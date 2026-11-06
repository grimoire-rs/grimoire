# shellcheck shell=bash
# Source this AFTER env.sh for README § Settings, global-scope steps:
#
#   source test/manual/scripts/env.sh
#   source test/manual/scripts/settings-env.sh
#
# Redirects every harness root a settings package writes into a gitignored
# directory under the rig, so no step touches a real ~/.claude, ~/.codex or
# ~/.copilot. Each root is created so global-scope detection sees the client.
# The project-scope steps need none of this: they write inside project-settings/.

if [ -n "${BASH_SOURCE[0]:-}" ]; then
    _grim_settings_script="${BASH_SOURCE[0]}"
else
    eval '_grim_settings_script="${(%):-%x}"'
fi
_grim_settings_dir="$(cd "$(dirname "$_grim_settings_script")/.." && pwd)/.settings-roots"

export CLAUDE_CONFIG_DIR="$_grim_settings_dir/claude"
export CODEX_HOME="$_grim_settings_dir/codex"
export COPILOT_HOME="$_grim_settings_dir/copilot"
mkdir -p "$CLAUDE_CONFIG_DIR" "$CODEX_HOME" "$COPILOT_HOME"

{
    echo "grimoire settings roots (redirected into the rig):"
    echo "  CLAUDE_CONFIG_DIR=$CLAUDE_CONFIG_DIR"
    echo "  CODEX_HOME=$CODEX_HOME"
    echo "  COPILOT_HOME=$COPILOT_HOME"
} >&2

unset _grim_settings_script _grim_settings_dir
