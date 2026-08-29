#!/bin/sh
# Install Press for the current user from its AppImage release.
#
#   curl -fsSL https://raw.githubusercontent.com/IgorVaryvoda/press/main/scripts/install.sh | sh
#   sh install.sh 0.3.14        # a specific version instead of the latest
#
# It installs one file and two desktop pieces, all inside $HOME:
#
#   ~/.local/bin/press
#   ~/.local/share/applications/press.desktop
#   ~/.local/share/icons/hicolor/512x512/apps/press.png
#
# Remove Press by deleting those three paths.
#
# The AppImage keeps its own updater: `press update` replaces the installed file
# in place and keeps its name, so nothing here has to run again.
#
# Trust root is HTTPS plus the release's own SHA256SUMS. If you want a package
# signed by a distribution, use the .deb or the AUR package instead.
set -eu

repo=${PRESS_REPO:-IgorVaryvoda/press}
bin_dir=${PRESS_BIN_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}
data_dir=${XDG_DATA_HOME:-$HOME/.local/share}
version=${1:-}

die() {
	echo "install.sh: $1" >&2
	exit 1
}

[ "$(uname -s)" = Linux ] || die "this installs the Linux AppImage; macOS uses Homebrew, Windows uses the .exe"
[ "$(uname -m)" = x86_64 ] || die "the AppImage is x86-64 only; build from source on $(uname -m)"
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"

# The /releases/latest URL redirects to the tag, so the version needs no API call
# and no JSON parsing.
if [ -z "$version" ]; then
	tag=$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest") ||
		die "could not reach GitHub"
	version=${tag##*/v}
fi
[ -n "$version" ] || die "could not work out which version to install"

asset="press_${version}_x86_64.AppImage"
base="https://github.com/$repo/releases/download/v${version}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading Press $version…"
curl -fsSL --retry 3 -o "$tmp/$asset" "$base/$asset" || die "could not download $asset"
curl -fsSL --retry 3 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "could not download SHA256SUMS"

# Check only our own line: the ledger covers every platform's assets.
(cd "$tmp" && grep " $asset\$" SHA256SUMS | sha256sum -c - >/dev/null) ||
	die "$asset does not match its published checksum; nothing was installed"

chmod +x "$tmp/$asset"
install -Dm755 "$tmp/$asset" "$bin_dir/press"

# The AppImage carries the same desktop entry and icon the .deb installs. Take
# them from the file just verified rather than writing a second copy by hand.
(cd "$tmp" &&
	./"$asset" --appimage-extract 'usr/share/applications/*.desktop' >/dev/null 2>&1 &&
	./"$asset" --appimage-extract 'usr/share/icons/hicolor/512x512/apps/*.png' >/dev/null 2>&1) ||
	echo "install.sh: could not read the bundled menu entry; the command still works" >&2

entry="$tmp/squashfs-root/usr/share/applications/press.desktop"
icon="$tmp/squashfs-root/usr/share/icons/hicolor/512x512/apps/press.png"
if [ -f "$entry" ] && [ -f "$icon" ]; then
	# The bundled Exec is a bare `press`, which a launcher resolves against its own
	# PATH rather than the shell's. Point it at the file actually installed.
	sed "s|^Exec=.*|Exec=$bin_dir/press %F|" "$entry" >"$tmp/press.desktop"
	install -Dm644 "$tmp/press.desktop" "$data_dir/applications/press.desktop"
	install -Dm644 "$icon" "$data_dir/icons/hicolor/512x512/apps/press.png"
	update-desktop-database "$data_dir/applications" >/dev/null 2>&1 || true
	gtk-update-icon-cache -tq "$data_dir/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Press $version is installed at $bin_dir/press"
case ":$PATH:" in
*":$bin_dir:"*) echo "Run: press ~/path/to/folder" ;;
*) echo "Add $bin_dir to your PATH, or run it as $bin_dir/press" ;;
esac
