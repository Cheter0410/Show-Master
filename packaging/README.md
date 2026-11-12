# Packaging

Gebaut wird lokal: Linux und Windows auf einem Linux-PC (Arch oder Debian/Ubuntu), macOS auf einem Mac.
Es gibt keine CI.

| Ziel | Datei | Wo / wie |
|---|---|---|
| macOS (Universal: Intel + Apple Silicon) | `ShowMaster-x.y.z.dmg` | Mac: `bash packaging/macos/setup-and-build.sh` |
| Windows | `ShowMaster-Setup-x.y.z.exe` (Inno Setup, `packaging/windows/showmaster.iss`) | Linux-PC: `./packaging/build-on-linux.sh` (Cross-Build mit MinGW, Inno Setup unter Wine) |
| Arch Linux | `showmaster-x.y.z-1-x86_64.pkg.tar.zst` (`packaging/arch/PKGBUILD`) | Linux-PC mit `makepkg` |
| Debian/Ubuntu | `showmaster_x.y.z-1_amd64.deb` | Linux-PC mit `cargo-deb` |
| jedes Linux | `showmaster-x.y.z-linux-x86_64.tar.gz` | Linux-PC |
| Updater-Patches | `showmaster-macos-arm64.gz`, `showmaster-macos-x86_64.gz`, `showmaster-windows-x86_64.gz` | entstehen mit DMG bzw. Windows-Build |

Alles landet in `dist/`. Die gebaute Weboberfläche (`ui/dist`) liegt im Repository; Node.js wird zum
Bauen nicht gebraucht.

## Veröffentlichen

Der Updater liest `https://github.com/Cheter0410/show-master-releases`. Ablauf und alle Befehle:
[`docs/release-anleitung.md`](../docs/release-anleitung.md).

## Nicht signiert

Für signierte und notarisierte macOS-Apps braucht es ein Apple-Developer-Zertifikat, für signierte
Windows-Installer ein Code-Signing-Zertifikat. Beides fehlt; die Mac-App ist ad hoc signiert. Folge: macOS
verlangt beim ersten Start „Dennoch öffnen“, Windows zeigt SmartScreen.

## Linux-Updates

Der Updater installiert unter Linux nicht selbst, sondern verweist auf die Download-Seite (Arch: neues Paket
mit `pacman -U` installieren).
