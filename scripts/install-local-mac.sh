#!/usr/bin/env bash
# Build, sign, install and verify a local OpenLogi build on this Mac.
#
# Fails fast and loud: every step either proves it worked or stops with the
# reason and the fix. Nothing in /Applications is touched until a complete,
# signed bundle exists; if anything fails after that, the previous app is put
# back automatically. Local-only tool for Shane's fork (not for upstream).
#
#   scripts/install-local-mac.sh              # tests + build + install + verify
#   scripts/install-local-mac.sh --skip-tests # when tests just passed
#
# App icon: the icon chosen in Settings is baked into the bundle at build time
# (the app would otherwise paste a Finder icon onto the signed bundle at every
# launch, which breaks the code seal). The choice is remembered in
# ~/.openlogi-port/baked-icon; delete that file to go back to the stock icon.
set -euo pipefail

IDENTITY="${OPENLOGI_LOCAL_IDENTITY:-Apple Development: Shane Grindle (8ADVFPZXXQ)}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
APP_SRC="$REPO/target/release/bundle/osx/OpenLogi.app"
APP_DST="/Applications/OpenLogi.app"
APP_NEW="/Applications/OpenLogi.app.new"
CONFIG="$HOME/.config/openlogi/config.toml"
LOG_DIR="$HOME/.local/state/openlogi"
STATE="$HOME/.openlogi-port"
BACKUPS="$STATE/backups"
SERVICE="gui/$(id -u)/org.openlogi.agent.service"
ENTS="$REPO/crates/openlogi-desktop/bundle/OpenLogi.entitlements"
SKIP_TESTS=0
if [[ "${1:-}" == "--skip-tests" ]]; then SKIP_TESTS=1; fi

step() { printf '\n==> %s\n' "$*"; }
die()  { printf '\n✗ FAILED: %s\n' "$*" >&2; exit 1; }
agent_log() { echo "$LOG_DIR/agent.$(date -u +%Y-%m-%d).log"; }

# ── preflight ───────────────────────────────────────────────────────────────
step "preflight"
[[ -f "$CONFIG" ]] || die "no config at $CONFIG"
security find-identity -v -p codesigning | grep -qF "$IDENTITY" \
  || die "signing identity not found: $IDENTITY (Keychain Access → My Certificates)"
command -v cargo-bundle >/dev/null || die "cargo-bundle missing: cargo install cargo-bundle"
/usr/bin/xcrun --find actool >/dev/null 2>&1 || die "actool missing: install Xcode 26+"
grep -q '^\[package.metadata.bundle.macos\]' "$REPO/crates/openlogi-desktop/Cargo.toml" \
  || die "cargo-bundle 0.12 metadata fix not applied to crates/openlogi-desktop/Cargo.toml"
if [[ ! -e "$REPO/target" ]]; then
  ln -s "$HOME/.cargo/shared-target" "$REPO/target"   # xtask hard-codes <repo>/target
fi
COMMIT="$(git -C "$REPO" rev-parse --short HEAD)"
BRANCH="$(git -C "$REPO" rev-parse --abbrev-ref HEAD)"
echo "branch $BRANCH @ $COMMIT, identity: $IDENTITY"

# ── tests ───────────────────────────────────────────────────────────────────
if (( SKIP_TESTS == 0 )); then
  step "tests"
  (cd "$REPO" && cargo test -q -p openlogi-core -p openlogi-device -p openlogi-hook \
      -p openlogi-agent-core -p openlogi-agent) || die "tests failed — nothing was installed"
fi

# ── build ───────────────────────────────────────────────────────────────────
step "bundle (production identity)"
(cd "$REPO" && cargo run -q -p xtask -- macos bundle --channel production) \
  || die "bundle build failed"
[[ -d "$APP_SRC" ]] || die "bundle not found at $APP_SRC"

# ── icon ────────────────────────────────────────────────────────────────────
# Valid config values are "openlogi" (stock) and the alternates in design/icon.
ICON="$(sed -n 's/^app_icon = "\(.*\)"$/\1/p' "$CONFIG" || true)"
ICON="${ICON:-openlogi}"
if [[ "$ICON" != "openlogi" ]]; then
  mkdir -p "$STATE" && echo "$ICON" > "$STATE/baked-icon"   # a fresh choice in Settings wins
elif [[ -f "$STATE/baked-icon" ]]; then
  ICON="$(cat "$STATE/baked-icon")"
fi
if [[ "$ICON" != "openlogi" ]]; then
  step "bake icon: $ICON"
  DOC="$REPO/design/icon/openlogi-$ICON.icon"
  [[ -d "$DOC" ]] || die "no Icon Composer document for '$ICON' at $DOC"
  WORK="$(mktemp -d)"
  /usr/bin/ditto "$DOC" "$WORK/AppIcon.icon"
  mkdir -p "$WORK/out"
  /usr/bin/xcrun actool "$WORK/AppIcon.icon" --compile "$WORK/out" --platform macosx \
    --minimum-deployment-target 13.0 --target-device mac --app-icon AppIcon \
    --output-partial-info-plist "$WORK/icon.plist" > "$WORK/actool.log" 2>&1 \
    || { cat "$WORK/actool.log" >&2; die "actool could not compile $DOC"; }
  [[ -f "$WORK/out/Assets.car" && -f "$WORK/out/AppIcon.icns" ]] \
    || { cat "$WORK/actool.log" >&2; die "actool produced no icon"; }
  cp "$WORK/out/Assets.car" "$APP_SRC/Contents/Resources/Assets.car"
  cp "$WORK/out/AppIcon.icns" "$APP_SRC/Contents/Resources/AppIcon.icns"
  rm -rf "$WORK"
fi

# ── sign inside-out (same order and entitlements as xtask's release signing) ─
step "codesign"
sign() {
  local out
  out="$(codesign --force --options runtime --timestamp=none "$@" --sign "$IDENTITY" 2>&1)" \
    || { echo "$out" >&2; die "codesign failed: ${*: -1}"; }
}
sign "$APP_SRC/Contents/Library/LoginItems/OpenLogi Agent.app"
sign "$APP_SRC/Contents/Library/LoginItems/OpenLogi Overlay.app"
sign --entitlements "$ENTS" "$APP_SRC/Contents/MacOS/openlogi"
sign --entitlements "$ENTS" "$APP_SRC"
codesign --verify --deep --strict "$APP_SRC" || die "signature does not verify"
codesign -dr - "$APP_SRC/Contents/Library/LoginItems/OpenLogi Agent.app" 2>&1 \
  | grep -qF "$IDENTITY" || die "agent designated requirement is not pinned to $IDENTITY"

# ── stage next to the live app, then swap ──────────────────────────────────
step "install"
STAMP="$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUPS"
cp -p "$CONFIG" "$BACKUPS/config-$STAMP.toml"
rm -rf "$APP_NEW"
/usr/bin/ditto "$APP_SRC" "$APP_NEW"
codesign --verify --deep --strict "$APP_NEW" || die "staged copy does not verify (live app untouched)"

osascript -e 'tell application "OpenLogi" to quit' >/dev/null 2>&1 || true
sleep 2
BACKUP=""
if [[ -d "$APP_DST" ]]; then
  BACKUP="$BACKUPS/OpenLogi-$STAMP.app"
  mv "$APP_DST" "$BACKUP"
fi
# From here on a failure puts the previous app and config back.
rollback() {
  printf '\n↩ rolling back to the previous app and config\n' >&2
  rm -rf "$APP_DST"
  if [[ -n "$BACKUP" && -d "$BACKUP" ]]; then mv "$BACKUP" "$APP_DST"; fi
  cp -p "$BACKUPS/config-$STAMP.toml" "$CONFIG"
  launchctl kickstart -k "$SERVICE" >/dev/null 2>&1 || true
  open -a "$APP_DST" >/dev/null 2>&1 || true
}
trap 'rc=$?; if (( rc != 0 )); then rollback; fi' EXIT
mv "$APP_NEW" "$APP_DST"

# The icon is baked in; "openlogi" (the stock value) makes the app leave the
# bundle's icon alone. Never write a value the config schema rejects.
sed -i '' 's/^app_icon = ".*"$/app_icon = "openlogi"/' "$CONFIG"
# A self-signed build must never be replaced by an auto-installed release.
sed -i '' 's/^auto_install_updates = true$/auto_install_updates = false/' "$CONFIG"

# ── restart + verify ────────────────────────────────────────────────────────
step "restart agent + app"
SINCE="$(date -u +%Y-%m-%dT%H:%M:%S)"
sleep 1   # nothing the old agent logs in this second can count as fresh
launchctl kickstart -k "$SERVICE" || die "could not restart $SERVICE"
open -a "$APP_DST"

step "verify"
ok=0
for _ in $(seq 1 30); do
  sleep 2
  # recomputed every pass: the UTC day can roll over while we wait
  recent="$(awk -v s="$SINCE" '$1 > s' "$(agent_log)" 2>/dev/null || true)"
  # only what the NEW agent logged counts
  recent="$(awk 'found || /openlogi-agent started/ {found=1; print}' <<<"$recent")"
  if grep -q "could not load config.toml" <<<"$recent"; then
    die "the agent rejected config.toml and fell back to defaults"
  fi
  if grep -q "Input Monitoring is NOT granted" <<<"$recent"; then
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
    die "Input Monitoring is off for OpenLogi Agent — turn it on in the pane just opened, then re-run with --skip-tests"
  fi
  if grep -q "control capture active" <<<"$recent"; then ok=1; break; fi
done
(( ok )) || die "the new agent did not capture the mouse within 60 s — see $(agent_log)"
sleep 3
if [[ -e "$APP_DST/Icon"$'\r' ]]; then die "a custom Finder icon was pasted onto the bundle (seal broken)"; fi
codesign --verify --strict "$APP_DST" || die "installed bundle fails strict verification"
trap - EXIT

step "done"
echo "✓ OpenLogi $BRANCH @ $COMMIT installed, signed, and capturing the mouse."
if [[ -n "$BACKUP" ]]; then echo "  previous app: $BACKUP"; fi
echo "  config backup: $BACKUPS/config-$STAMP.toml"
echo "  backups kept: $(ls -1d "$BACKUPS"/OpenLogi-*.app 2>/dev/null | wc -l | tr -d ' ') (prune by hand when you like)"
