#!/bin/sh
# Capture the `resume.json` a released kumo writes for `kumo update`, as an
# upgrade fixture for the next release:
#
#   sh capture.sh "$(command -v kumo)" resume-$(kumo --version | cut -d' ' -f2).json
#
# The released daemon builds a known layout (two sessions, a second tab, a
# split, an aliased agent pane, scrollback), then `kumo server restart` makes
# it write the resume file and exec its own binary. That binary is swapped for
# a shim that copies the file before handing off to the real one.
set -eu
released="$1"
out="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
root="$(mktemp -d)"
trap 'kumo_ kill >/dev/null 2>&1 || true; rm -rf "$root"' EXIT
mkdir "$root/bin" "$root/ws"
cp "$released" "$root/bin/kumo-released"
cp "$released" "$root/bin/kumo"
printf 'shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n' > "$root/config"
export KUMO_CONFIG_DIR="$root" XDG_RUNTIME_DIR="$root" XDG_STATE_HOME="$root" KUMO_NO_UPDATE=1
kumo_() { "$root/bin/kumo-released" "$@"; }

"$root/bin/kumo" daemon "$root/ws" >/dev/null 2>&1 &
sleep 1.5
kumo_ pane send-keys -p 1 echo Space FIXTURE_SCROLLBACK Enter >/dev/null
kumo_ pane split --horizontal >/dev/null
kumo_ agent spawn cat >/dev/null
ai=$(kumo_ agent status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["agents"][0]["pane_id"])')
kumo_ agent rename "$ai" worker >/dev/null
kumo_ tab new "$root/ws" --name second >/dev/null
kumo_ session new "$root/ws" --name other >/dev/null
sleep 0.5

cat > "$root/bin/kumo" <<SHIM
#!/bin/sh
cp "\$3" "$out"
exec "$root/bin/kumo-released" "\$@"
SHIM
chmod +x "$root/bin/kumo"
kumo_ server restart >/dev/null
sleep 2
test -s "$out"
echo "wrote $out"
