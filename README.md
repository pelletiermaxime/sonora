<div align="center">

<img src="./.github/readme-icon.svg" alt="" width="60" height="60">

<h1>Sonora — Max's metal discovery fork</h1>

<p align="center">
  <a href="./CHANGELOG.md">Fork changelog</a>
  ·
  <a href="https://github.com/sonorahq/sonora">Original Sonora</a>
  ·
  <a href="https://sonorahq.org/docs">Upstream docs</a>
  ·
  <a href="https://github.com/pelletiermaxime/sonora/releases">Fork releases</a>
</p>

[![Build](https://img.shields.io/github/actions/workflow/status/pelletiermaxime/sonora/ci.yml?style=flat-square&label=build)](https://github.com/pelletiermaxime/sonora/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/pelletiermaxime/sonora?style=flat-square&label=license)](./COPYING)

### A native music streaming client, built with Rust and GPUI

Stream from your favorite services and play local files — all in one **native** app.
</div>

## About this fork

This is [Max's personal fork](https://github.com/pelletiermaxime/sonora) of
[Sonora by sonorahq](https://github.com/sonorahq/sonora). Sonora's contributors created the
native player, streaming integrations, and design system that this fork builds on. The
additional features focus on discovering metal and using my Last.fm history; they may stay
specific to this fork rather than being proposed upstream.

Compared with upstream, this fork adds:

- **Metal release discovery:** browse a week of Metal Archives releases, with full-length,
  all-release, familiar-band, and New to you filters. Data loads automatically and is cached
  for 24 hours. New to you checks bands with at most five personal plays and at least 25,000
  Last.fm listeners.
- **Artist discographies:** load Metal Archives data on demand, separate main albums from
  singles and EPs, see prominent review averages, and open album and review pages in the
  built-in browser. Matching provider covers and hover play controls keep playback in Sonora.
- **Listening charts:** browse and search your all-time top Last.fm artists and albums in
  Discover's Your listening tab, with prominent play counts and the last update time.
  Open artist and album pages through matching links in your active music provider.
  Opening the tab reads your shared cache without making additional requests.
- **Personal listening stats:** show artist plays and genre in the artist header, plus personal
  album plays from a shared Last.fm library imported once. Daily refreshes fetch recent scrobbles
  and update totals only for the albums you listened to, catching up after time away. Generic
  title matching handles punctuation, subtitles, and small spelling differences while leaving
  ambiguous matches unavailable. Edits to older scrobbles are reflected when that album is
  played again; daily sync does not reread the full historical chart.
- **Metal genre and similar-band browsing:** explore [empath](https://www.furia.com/em/index.html)
  genre rankings in Discover and related bands on artist pages, with source snapshot dates.
- **Upcoming releases:** fetch exact dates for recent releases in the background and show
  upcoming dates instead of offering playback for unreleased albums.

Metal Archives requests are spaced two seconds apart and limited to 100 per day. A temporary
native browser handles verification or band selection when necessary; the site may still
refuse a request. Successful results are cached for 24 hours, and a failed refresh retains them.
The fork does not provide an official Metal Archives API.

Connect Last.fm in **Settings → Scrobbling** using your Last.fm API account to use personal
listening history. Playback still uses your selected streaming provider and its access rules.
The app, executable, configuration paths, and package identifiers currently retain the upstream
`Sonora` / `sonora` names, so this fork shares settings with an upstream installation.

The screenshots below show the upstream player; they are not screenshots of the added metal features.

<div align="center">
    <table>
      <tr>
        <td colspan="2">
          <img width="1613" height="981" alt="image" src="https://github.com/user-attachments/assets/7952a912-7fbc-4186-b467-a08dd7e71e22" />
        </td>
      </tr>
      <tr>
        <td width="50%">
          <img width="1623" height="987" alt="image" src="https://github.com/user-attachments/assets/580bf9d6-db85-4fde-b599-82ba2a28cc51" />
        </td>
        <td width="50%">
          <img width="1623" height="987" alt="image" src="https://github.com/user-attachments/assets/64fcd709-5917-432c-a418-2e07527343d2" />
        </td>
      </tr>
    </table>
</div>
<div align="center">
    <sub>
      Adaptive themes are optional. Everything is (or will be) customizable.
    </sub>
</div>

> [!IMPORTANT]
> **Sonora is not a piracy tool.**
>
> Sonora is not a platform for obtaining or sharing copyrighted material. We will not implement any functions that can be used to export decrypted streams, DRM licenses, content keys, or to convert protected streams into media files.
>
> Sonora is not designed to circumvent subscriptions or other restrictions put in place by music streaming platforms. If the service demands that you have a valid subscription in order to play back their tracks, so will Sonora.
>
> Features aimed at ripping, downloading, distributing, or gaining access to protected streaming content are out of scope for the project.

## Features

- **Apple Music, Spotify, YouTube Music, Deezer, Subsonic/Navidrome** and local playback
- Gapless playback, audio normalization, shuffle, sleep timer
- Synced/karaoke lyrics, background vocals, and romanization
- Scrobbling with LastFM, ListenBrainz, LibreFM, and Maloja
- Themes, fonts, icons, transparency, blur, and window styling
- Discord Rich Presence, native file opening
- macOS, Windows, Linux, and (probably) FreeBSD support

## Installation

Download builds from [this fork's releases](https://github.com/pelletiermaxime/sonora/releases).
Upstream Homebrew, AUR, Flathub/repository, and Nix binary packages install upstream Sonora and
do not contain this fork's additions. Until a fork release is published, build from source or
download artifacts from a successful manual release workflow run.

### macOS

Download the fork's `sonora-*-macos.dmg`, then drag `Sonora.app` into Applications. The bundle
contains both Apple Silicon and Intel executables. It is ad hoc signed and not notarized.
If macOS quarantines the downloaded app, remove quarantine after checking its source:

```sh
xattr -dr com.apple.quarantine /Applications/Sonora.app
```

### Linux

#### Flatpak

Download the `.flatpak` bundle for your architecture from this fork and install it locally:

```sh
flatpak install --user ./sonora-*.flatpak
```

Fork bundles are unsigned and are not published to upstream's signed update repository.
Install a newly downloaded fork bundle when updating.

#### AppImage

Download the `x86_64` AppImage from the
[fork releases](https://github.com/pelletiermaxime/sonora/releases), make it executable and run
it:

```sh
chmod +x sonora-*.AppImage
./sonora-*.AppImage
```

An `aarch64` build is published beside it. The AppImage carries no Vulkan driver and no ALSA
bridge, so both still come from your system. It does not update itself, but it carries its update
information, so [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate) or an
AppImage manager such as [AppManager](https://github.com/kem-a/AppManager) can fetch a new release
for you.

Linux native builds also need an ALSA bridge matching the sound server (`pipewire-alsa` or
`pulseaudio-alsa`) and a Vulkan driver. The built-in browser needs WebKitGTK 4.1 or 4.0 on
native Linux installations; Flatpak supplies WebKit through its GNOME runtime.

### Windows

#### Installer

Download `Sonora-Setup.exe` from [this fork's releases](https://github.com/pelletiermaxime/sonora/releases),
or `Sonora-Setup-arm64.exe` for Windows on ARM. Windows releases are unsigned.

#### Portable

Download the `windows-msvc.exe` for your architecture from [fork releases](https://github.com/pelletiermaxime/sonora/releases).

## Development and release builds

```sh
git clone https://github.com/pelletiermaxime/sonora.git
cd sonora
nix develop                    # optional: provides native development dependencies
cargo run --locked --package sonora
```

The Linux Cargo configuration uses mold; install it or run with `RUSTFLAGS=""` if unavailable.
The Nix flake's default package points to an upstream released binary, so use Cargo to build
the checked-out fork instead of `nix build` when developing.

The inherited [release workflow](https://github.com/pelletiermaxime/sonora/actions/workflows/release.yml)
builds Linux x86_64 and ARM64 binaries, AppImages and Flatpak bundles, a universal macOS DMG,
and Windows x86_64 and ARM64 executables and installers. It reuses Rust and sccache caches.

- **Build without publishing:** run the `release` workflow manually, select the branch to build,
  and download its artifacts when all jobs finish. Branch builds use the commit hash in filenames.
- **Publish a version:** after validating the fork, update the workspace version and changelog,
  then push the matching `v*` tag. The first fork release is `v1.0.0`. Tags containing a
  hyphen are marked as GitHub prereleases. A successful run publishes packages, SHA-256
  checksums, and license notices to this fork's GitHub Releases.

GitHub Actions must be enabled on the fork. Builds and release uploads use the repository's
`GITHUB_TOKEN`; fork releases do not require upstream's Flatpak GPG key or announcement secrets.
The original signed Flatpak repository and Discord release announcements remain upstream-only.

## Community

Sonora's upstream community is on [Discord](https://discord.gg/a8N8Tx23rV) and
[Matrix](https://matrix.to/#/#sonora:nolight.dev). Report problems with this fork's additions
in this repository rather than asking upstream to support the custom features.

## AI policy

We have nothing against the usage of LLMs in the project — in fact, we use them ourselves.
We believe that AI can speed up development in a lot of meaningful ways and be a useful
tool for learning new concepts. We have also found it particularly helpful for
contributing to substantial codebases such as GPUI and librespot, where it has helped
us quickly locate the relevant parts of the code.

**However**, using AI cannot act as an excuse for failing to
understand, review, and test the changes proposed. Furthermore, we expect communication
with a real person, not a computer. This includes but is not limited to PR/issue text
generation, comments in discussions, etc. A short summary of minor changes can be
generated and does not need to be disclosed explicitly, but the reasoning and motivation
behind a change must come from the contributor and reflect their own understanding.

Note that PRs that fail to adhere to these requirements may be rejected without further notice.

AI-assisted proofreading and translation of human-written text are permitted.

## Translations

<!-- i18n:start -->

| Language | Translated | Coverage |
| --- | --- | --- |
| English (`en-US`) | 873/873 | 100% |
| Deutsch (`de`) | 628/873 | 72% |
| Español (`es`) | 690/873 | 79% |
| Français (`fr`) | 749/873 | 86% |
| Italiano (`it`) | 605/873 | 69% |
| Bahasa Indonesia (`id`) | 605/873 | 69% |
| 日本語 (`ja`) | 605/873 | 69% |
| Русский (`ru`) | 711/873 | 81% |
| Українська (`uk`) | 711/873 | 81% |
| Polski (`pl`) | 711/873 | 81% |
| Čeština (`cs`) | 743/873 | 85% |
| Português (Brasil) (`pt-BR`) | 605/873 | 69% |
| 简体中文 (`zh-CN`) | 605/873 | 69% |
| Türkçe (`tr`) | 605/873 | 69% |
| Shqip (`sq`) | 714/873 | 82% |

<!-- i18n:end -->

## Star History

<a href="https://www.star-history.com/?repos=sonorahq%2Fsonora&type=date&logscale=&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=sonorahq/sonora&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=sonorahq/sonora&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=sonorahq/sonora&type=date&legend=top-left" />
 </picture>
</a>

## Credits

Sonora is built with the help of some incredible open-source projects, including:

- [Zed](https://github.com/zed-industries/zed) — a wonderful editor (~~ab~~)used by all core team members. Conveniently provides `gpui` — their native Rust rendering stack.
- [librespot](https://github.com/librespot-org/librespot) — Spotify playback and library integration.
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) — certain YouTube ideas implemented in [ytmusic-rs](https://github.com/sonorahq/ytmusic-rs) :)

## Code signing

This fork's Windows releases are unsigned. macOS bundles use ad hoc signing and are not
notarized. Upstream's signing applications and certificates do not apply to this fork.

## License

Copyright (C) 2026 Sonora Contributors.

Sonora is free software, released under the [GNU General Public License version
3 or later](COPYING).

Sonora is an unofficial client and is not affiliated with, endorsed by, or
sponsored by Spotify AB.

The binary also embeds the [Inter](https://github.com/rsms/inter) typeface (SIL
Open Font License 1.1) and four interchangeable icon sets:
[Lucide](https://lucide.dev) (ISC), [Iconoir](https://iconoir.com) (MIT),
[Remix Icon](https://remixicon.com) 4.8.0 (Apache 2.0) and the
[Solar](https://www.figma.com/community/file/1166831539721848736) Linear set
(CC BY 4.0, by 480 Design). Each pack keeps its licence beside its files in
`assets/icons`. `THIRD-PARTY.md` lists every bundled dependency.
