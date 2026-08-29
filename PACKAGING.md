# Package-manager publishing

Press has one release build. Homebrew, AUR, APT, and Chocolatey must wrap the
artifacts from that tagged GitHub release; they must not compile four independent
copies of the application.

| Channel | Package | Release artifact |
|---|---|---|
| Homebrew | Cask `press` | `Press_VERSION_aarch64.dmg` and `Press_VERSION_x64.dmg` |
| AUR | `press-bin` | `press_VERSION_x86_64.tar.gz` |
| APT | `press` | `press_VERSION_amd64.deb` |
| Chocolatey | `press` | `press_VERSION_x64-setup.exe` |

Linux and Windows are x86-64 only until the release workflow publishes another
architecture. The macOS Cask must carry separate Apple Silicon and Intel checksums.

Package registries, an APT host, and their repositories are outside this repository.
Confirm that external publishing is authorised before creating them or changing DNS.

## Prepare the release once

The current release job already creates signed AppImage, DMG, and NSIS installers.
Before the first package-manager release:

1. Add a real `authors` entry to `[package]` in `Cargo.toml`. `cargo-packager`
   uses it for Debian's `Maintainer` and the generated `PKGBUILD`; do not ship an
   empty value.
2. Add `[package.metadata.packager.deb]` and
   `[package.metadata.packager.pacman]` settings for runtime dependencies and
   the MIT licence. The Debian package should install its copyright file at
   `/usr/share/doc/press/copyright`; the Pacman archive should install
   `LICENSE` at `/usr/share/licenses/press/LICENSE`.
3. Determine Debian dependencies from the Ubuntu release binary with
   `dpkg-shlibdeps`, then keep the explicit list in the packager metadata. Do not
   infer it from this Arch workstation. Validate the list again whenever native
   linkage changes.
4. Start the Pacman dependency list with the packages that own every directly
   linked library, then verify it with `namcap`. On the current Arch host those
   owners are `dav1d`, `glibc`, `libavif`, `libgcc`, `libxcb`, `libxkbcommon`,
   and `libxkbcommon-x11`.
5. Change the Linux release matrix format from `appimage` to
   `appimage,deb,pacman`. Upload and publish these additional outputs:

   ```text
   dist/press_*_amd64.deb
   dist/press_*_x86_64.tar.gz
   ```

   Keep `update_glob: "*.AppImage"` and `update_format: appimage`. Native Linux
   packages contain a loose binary, so `src/update.rs` leaves updates to the
   package manager. The AppImage remains the Linux self-update payload.
6. Publish a `SHA256SUMS` file generated from the final release assets. Homebrew,
   AUR, and Chocolatey must all use hashes from the same completed release.

The expected native outputs from `cargo-packager 0.11.8` are:

```text
press_VERSION_amd64.deb
press_VERSION_x86_64.tar.gz
PKGBUILD
```

The generated `PKGBUILD` is a useful inspection artifact, not the AUR recipe:
the AUR package uses the required `press-bin` name and its own `.SRCINFO`.

### AppImage download

The AppImage itself is executable when built, but a browser download does not apply
that Unix mode. Document the required first-run step beside every AppImage link:

```bash
chmod +x press_*.AppImage
./press_*.AppImage
```

The graphical equivalent on Ubuntu is **Properties → Permissions → Allow executing
file as program**. Do not wrap the AppImage in another archive to carry its mode;
AppImage's distribution guide advises against that because it breaks integration.
The `.deb` and APT channel are the no-`chmod` path for Ubuntu users.

### Native Linux release gate

Run this on the Ubuntu release runner or an equivalent clean environment:

```bash
package_dir=$(mktemp -d)
cargo packager --release --formats deb,pacman --out-dir "$package_dir"
dpkg-deb --field "$package_dir"/press_*_amd64.deb \
  Package Version Architecture Maintainer Depends
dpkg-deb --contents "$package_dir"/press_*_amd64.deb
tar -tzf "$package_dir"/press_*_x86_64.tar.gz
```

Reject the release if `Maintainer` or `Depends` is empty, the licence is absent,
the desktop entry/icon is missing, or either archive lacks `/usr/bin/press`.
Build native packages on the oldest Linux release that Press claims to support.
An Ubuntu 24.04 build must not be advertised as supporting older glibc releases
until it is tested there.

## Publish a version

Package only a completed tag, never the current branch. A commit can land after a
tag while `Cargo.toml` still carries the old version.

1. Wait for `.github/workflows/release.yml` to finish successfully.
2. Confirm that the tag matches the version in the tagged `Cargo.toml`.
3. Confirm that the GitHub release contains every artifact in the table and
   `SHA256SUMS`.
4. Download the assets once and compare their SHA-256 hashes with `SHA256SUMS`.
5. Update and test each package recipe below.
6. Test a fresh install, an upgrade from the prior package, and uninstall.
7. Publish the package recipes only after those checks pass.
8. Update README install commands only after the public registries return the
   released version.

Do not create a new Press release solely to repair a package recipe. Use the
registry's package revision when possible (`pkgrel` on AUR, a Chocolatey package
fix version, or a Debian revision).

## Homebrew Cask

Use a personal tap first: a GitHub repository named
`IgorVaryvoda/homebrew-press`, containing `Casks/press.rb`. Submit to the main
Homebrew Cask repository only when its acceptance requirements are met.

Replace the version and both hashes in this template:

```ruby
cask "press" do
  arch arm: "aarch64", intel: "x64"

  version "VERSION"
  sha256 arm: "ARM64_SHA256", intel: "X64_SHA256"

  url "https://github.com/IgorVaryvoda/press/releases/download/v#{version}/Press_#{version}_#{arch}.dmg"
  name "Press"
  desc "Audit and optimise images locally"
  homepage "https://imageguide.dev/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true

  app "Press.app"
  binary "#{appdir}/Press.app/Contents/MacOS/press"
end
```

The Cask declares `auto_updates true` because the signed application already has
an in-app updater. Do not strip, re-sign, or rebuild the notarised DMG.

Test on both macOS architectures:

```bash
brew tap IgorVaryvoda/press
brew trust --cask IgorVaryvoda/press/press
brew audit --cask --strict press
brew install --cask IgorVaryvoda/press/press
press --help
open -a Press
brew uninstall --cask press
```

Also open a real folder in the application. A successful CLI probe alone does not
prove that Gatekeeper, the app bundle, or GPUI works.

## AUR

Publish a binary recipe at `ssh://aur@aur.archlinux.org/press-bin.git`. AUR rules
require the `-bin` suffix because this recipe consumes a prebuilt release archive.

Use this `PKGBUILD`, replacing the version and hash:

```bash
# Maintainer: MAINTAINER
pkgname=press-bin
pkgver=VERSION
pkgrel=1
pkgdesc='Audit and optimise images locally'
arch=('x86_64')
url='https://imageguide.dev/'
license=('MIT')
depends=('dav1d' 'glibc' 'hicolor-icon-theme' 'libavif' 'libgcc' 'libxcb' 'libxkbcommon' 'libxkbcommon-x11')
provides=('press')
conflicts=('press')
options=('!strip')
source=("press-${pkgver}-${CARCH}.tar.gz::https://github.com/IgorVaryvoda/press/releases/download/v${pkgver}/press_${pkgver}_${CARCH}.tar.gz")
sha256sums=('X86_64_SHA256')

package() {
  cp -a "${srcdir}/usr" "${pkgdir}/"
  mv "${pkgdir}/usr/share/licenses/press" "${pkgdir}/usr/share/licenses/${pkgname}"
}
```

Do not put the root-owned AppImage in `/opt`: its in-app updater would try to
replace a package-manager-owned file. The native archive installs the loose binary,
for which Press deliberately disables its updater.

Build and verify before pushing:

```bash
makepkg --cleanbuild
makepkg --printsrcinfo > .SRCINFO
namcap PKGBUILD press-bin-*.pkg.tar.zst
sudo pacman -U press-bin-*.pkg.tar.zst
press --help
pacman -Ql press-bin
sudo pacman -Rns press-bin
```

Launch the real window once on current Arch. Commit only `PKGBUILD`, `.SRCINFO`,
and any deliberate package-owned files, then push to the AUR remote. Regenerate
`.SRCINFO` after every metadata change.

## Debian package and APT repository

The `.deb` is useful by itself:

```bash
sudo apt install ./press_VERSION_amd64.deb
press --help
sudo apt remove press
```

A real `sudo apt install press` requires a signed repository. Host it separately
from the application repository, for example at `packages.imageguide.dev`, with
`dists/` and `pool/` at its web root.

Create an APT-only OpenPGP signing key. Keep the private key in the publishing
environment; publish only the public key and its full fingerprint. With `reprepro`,
the repository's `conf/distributions` is:

```text
Origin: Press
Label: Press
Codename: stable
Architectures: amd64
Components: main
Description: Press packages
SignWith: APT_SIGNING_KEY_FINGERPRINT
```

Add the completed release package and export the public key:

```bash
reprepro -b public includedeb stable press_VERSION_amd64.deb
gpg --export APT_SIGNING_KEY_FINGERPRINT > public/press-archive-keyring.gpg
reprepro -b public check
```

Publish `public/` atomically. Never publish unsigned `Release` metadata and never
store the private key in this repository.

Document client setup only after the hostname and fingerprint are final:

```bash
curl -fsSLo press-archive-keyring.gpg \
  https://packages.imageguide.dev/press-archive-keyring.gpg
gpg --show-keys --fingerprint press-archive-keyring.gpg
sudo install -m 0644 press-archive-keyring.gpg \
  /usr/share/keyrings/press-archive-keyring.gpg
echo 'deb [arch=amd64 signed-by=/usr/share/keyrings/press-archive-keyring.gpg] https://packages.imageguide.dev stable main' \
  | sudo tee /etc/apt/sources.list.d/press.list
sudo apt update
sudo apt install press
apt-cache policy press
```

The published documentation must show the expected fingerprint beside these
commands so the downloaded key can be checked out of band. Test install, upgrade,
and removal in clean Ubuntu environments at the oldest and newest supported
versions.

## Chocolatey

Create a `press` package that downloads the existing x64 NSIS installer from the
GitHub release. Do not embed or modify the installer.

The package needs `press.nuspec` plus `tools/chocolateyInstall.ps1`. The install
script is:

```powershell
$ErrorActionPreference = 'Stop'

$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  fileType       = 'exe'
  url64bit       = 'https://github.com/IgorVaryvoda/press/releases/download/vVERSION/press_VERSION_x64-setup.exe'
  checksum64     = 'X64_SHA256'
  checksumType64 = 'sha256'
  silentArgs     = '/S'
  validExitCodes = @(0)
}

Install-ChocolateyPackage @packageArgs
```

The NSIS installer registers `Press` in Programs and Features, so start with
Chocolatey's automatic uninstaller. Add `chocolateyUninstall.ps1` only if an actual
clean-VM uninstall test proves that automatic removal is insufficient.

Build and test in a disposable Windows VM:

```powershell
choco pack
choco install press --source . --yes --debug --verbose
choco uninstall press --yes --debug --verbose
```

After installation, launch Press from the Start menu, audit a real folder, and
confirm that Programs and Features reports the right version. The current NSIS
installer does not promise to add `press.exe` to `PATH`; fix that in the shared
installer before documenting a Chocolatey-only CLI command.

Push the tested `.nupkg` to the Chocolatey Community Repository and wait for its
automated checks and moderation to complete before advertising the command.

## Automation boundary

Manually dispatch the first publish. Once all four channels have completed one
install/upgrade/uninstall cycle, connect release tags only to the repetitive version,
URL, checksum, `.SRCINFO`, and repository-index updates. Keep registry credentials
and the APT private key in their package repositories, not in this application
repository.

Every automated update must still stop when the GitHub release is incomplete, a
checksum differs, a native package has empty dependency metadata, or a clean install
test fails.

## References

- [Homebrew Cask Cookbook](https://docs.brew.sh/Cask-Cookbook)
- [Homebrew tap trust](https://docs.brew.sh/Tap-Trust)
- [AppImage quickstart](https://docs.appimage.org/introduction/quickstart.html)
- [Distributing AppImages](https://docs.appimage.org/packaging-guide/distribution.html)
- [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines)
- [Arch `.SRCINFO`](https://wiki.archlinux.org/title/.SRCINFO)
- [Debian repository setup](https://wiki.debian.org/DebianRepository/Setup)
- [Chocolatey package creation](https://docs.chocolatey.org/en-us/create/create-packages/)
- [Chocolatey automatic uninstaller](https://docs.chocolatey.org/en-us/choco/features/auto-uninstaller/)
