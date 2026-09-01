#!/bin/sh
# Install Press for the current user from its AppImage release.
#
#   curl -fsSL https://raw.githubusercontent.com/IgorVaryvoda/press/main/scripts/install.sh | sh
#   sh install.sh 0.3.14        # a specific version instead of the latest
#
# PRESS_REPO, PRESS_BIN_DIR and PRESS_BASE_URL override the source repository, the
# install directory and the URL the assets come from.
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
# Trust root, in order of preference:
#
#   * the release's minisign signature, checked against the public key below —
#     the same key the AppImage's own updater trusts. This needs `minisign` or
#     `rsign` on PATH.
#   * without either of those, HTTPS plus the release's own SHA256SUMS, which
#     proves the download matches a ledger published by the same release: it
#     catches a corrupt download, not a substituted release. The script says so.
#
# If you want a package signed by a distribution, use the .deb or the AUR package.
set -eu

repo=${PRESS_REPO:-IgorVaryvoda/press}
# Verbatim second line of assets/updater.pub, once base64-decoded. Keep the two
# in step: a release is signed once, for the updater and for this script both.
pubkey=RWQdhW74368EBlimGIAtL0T8t8bBHOnUNIzS6uK55s8ib8c6K1wNs/64
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
base=${PRESS_BASE_URL:-"https://github.com/$repo/releases/download/v${version}"}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading Press $version…"
curl -fsSL --retry 3 -o "$tmp/$asset" "$base/$asset" || die "could not download $asset"
curl -fsSL --retry 3 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "could not download SHA256SUMS"

# Check only our own line: the ledger covers every platform's assets.
(cd "$tmp" && grep " $asset\$" SHA256SUMS | sha256sum -c - >/dev/null) ||
	die "$asset does not match its published checksum; nothing was installed"

# The ledger and the AppImage come from the same release page, so anyone able to
# replace one can replace the other. The signature is what an attacker cannot
# forge, so check it whenever the machine has something that can.
if command -v minisign >/dev/null 2>&1; then
	verifier=minisign
elif command -v rsign >/dev/null 2>&1; then
	verifier=rsign
else
	verifier=
fi

if [ -n "$verifier" ]; then
	curl -fsSL --retry 3 -o "$tmp/$asset.sig" "$base/$asset.sig" ||
		die "could not download $asset.sig"
	# cargo-packager base64-wraps the minisign signature file; both tools want it raw.
	base64 -d <"$tmp/$asset.sig" >"$tmp/$asset.minisig" ||
		die "$asset.sig is not the published signature format; nothing was installed"

	# cargo-packager signs a trusted comment of `timestamp:<secs><TAB>file:<name>`,
	# and the signature covers it. Without this check a genuine signature over an
	# older AppImage passes when that AppImage is served under a newer version
	# number. The comment is only worth anything once the verifier below has
	# checked it, so it takes both: this reads the claim, the verifier proves it.
	tab=$(printf '\t')
	signed=$(sed -n "s/^trusted comment:.*${tab}file://p" "$tmp/$asset.minisig" | head -n 1)
	[ "$signed" = "$asset" ] ||
		die "the signature is for ${signed:-no named file}, not $asset; nothing was installed"

	case $verifier in
	minisign) minisign -V -q -m "$tmp/$asset" -x "$tmp/$asset.minisig" -P "$pubkey" ;;
	rsign) rsign verify -P "$pubkey" -x "$tmp/$asset.minisig" "$tmp/$asset" ;;
	esac >/dev/null ||
		die "$asset is not signed by the Press release key; nothing was installed"
else
	echo "install.sh: no minisign or rsign on PATH, so the release signature was not checked." >&2
	echo "install.sh: only the release's own SHA256SUMS was verified, which does not prove authenticity." >&2
	echo "install.sh: install minisign (apt/dnf/brew install minisign, or cargo install rsign2) and re-run to verify it." >&2
fi

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
