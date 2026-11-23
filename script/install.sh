#!/usr/bin/env sh
set -eu

# Downloads a Wu Plus release from GitHub and unpacks it into ~/.local/.
# Optional: WU_VERSION=1.2.3 picks a release, WU_BUNDLE_PATH installs a local package.
# The ZED_ names of these variables still work for older instructions.

cleanup() {
    if [ "$dmg_mounted" = true ]; then
        hdiutil detach "$mount_point" >/dev/null || return
    fi
    if [ -n "$mount_point" ] && [ -d "$mount_point" ]; then
        rmdir "$mount_point"
    fi
    rm -rf "$temp"
}

main() {
    platform="$(uname -s)"
    arch="$(uname -m)"
    channel="${WU_CHANNEL:-${ZED_CHANNEL:-stable}}"
    suffix=""
    app_id="io.github.chenqinru.wu-plus"
    mac_app="Wu Plus.app"
    case "$channel" in
        stable) ;;
        dev)
            suffix="-dev"
            app_id="$app_id-dev"
            mac_app="Wu Plus Dev.app"
            ;;
        *) echo "Unsupported release channel $channel" >&2; exit 1 ;;
    esac
    repo="chenqinru/wu-plus"
    version="${WU_VERSION:-${ZED_VERSION:-latest}}"
    bundle_path="${WU_BUNDLE_PATH:-${ZED_BUNDLE_PATH:-}}"
    if [ "$version" = "latest" ]; then
        download_base="https://github.com/$repo/releases/latest/download"
    else
        download_base="https://github.com/$repo/releases/download/wu-plus-v$version"
    fi
    # Use TMPDIR if available (for environments with non-standard temp directories)
    if [ -n "${TMPDIR:-}" ] && [ -d "${TMPDIR}" ]; then
        temp="$(mktemp -d "$TMPDIR/wu-plus-XXXXXX")"
    else
        temp="$(mktemp -d "/tmp/wu-plus-XXXXXX")"
    fi
    dmg_mounted=false
    mount_point=""
    trap cleanup EXIT

    if [ "$platform" = "Darwin" ]; then
        platform="macos"
    elif [ "$platform" = "Linux" ]; then
        platform="linux"
    else
        echo "Unsupported platform $platform"
        exit 1
    fi

    case "$platform-$arch" in
        macos-arm64* | linux-arm64* | linux-aarch64)
            arch="aarch64"
            ;;
        macos-x86* | linux-x86*)
            arch="x86_64"
            ;;
        *)
            echo "Unsupported platform or architecture"
            exit 1
            ;;
    esac

    if [ -z "$bundle_path" ]; then
        if command -v curl >/dev/null 2>&1; then
            curl () {
                command curl -fL "$@"
            }
        elif command -v wget >/dev/null 2>&1; then
            curl () {
                wget -O- "$@"
            }
        else
            echo "Could not find 'curl' or 'wget' in your path"
            exit 1
        fi
    fi

    "$platform" "$@"

    if [ "$(command -v wu-plus || true)" = "$HOME/.local/bin/wu-plus" ]; then
        echo "Wu Plus has been installed. Run with 'wu-plus'"
    else
        echo "To run Wu Plus from your terminal, you must add ~/.local/bin to your PATH"
        echo "Run:"

        case "${SHELL:-sh}" in
            *zsh)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.zshrc"
                echo "   source ~/.zshrc"
                ;;
            *fish)
                echo "   fish_add_path -U $HOME/.local/bin"
                ;;
            *)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.bashrc"
                echo "   source ~/.bashrc"
                ;;
        esac

        echo "To run Wu Plus now, '~/.local/bin/wu-plus'"
    fi
}

linux() {
    if [ -n "$bundle_path" ]; then
        cp "$bundle_path" "$temp/wu-plus-linux-$arch.tar.gz"
    else
        echo "Downloading Wu Plus version: $version"
        curl "$download_base/wu-plus-linux-$arch.tar.gz" > "$temp/wu-plus-linux-$arch.tar.gz"
    fi

    app="wu-plus$suffix.app"
    install_dir="$HOME/.local/$app"
    mkdir "$temp/unpacked"
    tar -xzf "$temp/wu-plus-linux-$arch.tar.gz" -C "$temp/unpacked"
    test -x "$temp/unpacked/$app/bin/wu-plus"
    test -x "$temp/unpacked/$app/libexec/wu-editor"
    test -f "$temp/unpacked/$app/share/applications/$app_id.desktop"
    mkdir -p "$HOME/.local"
    rm -rf "$install_dir"
    mv "$temp/unpacked/$app" "$install_dir"

    wu_editor="$install_dir/libexec/wu-editor"
    if [ -f "$wu_editor" ] && command -v ldd >/dev/null 2>&1; then
        missing="$(ldd "$wu_editor" 2>/dev/null | sed -n 's/^[[:space:]]*\(.*\) => not found$/\1/p')"
        if [ -n "$missing" ]; then
            echo "Warning: your system is missing libraries that Wu Plus needs:"
            echo "$missing" | sed 's/^/    /'
            echo "Install them with your package manager, or Wu Plus will fail to start."
        fi
    fi

    # Setup ~/.local directories
    mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"

    # Link the binary
    ln -sf "$install_dir/bin/wu-plus" "$HOME/.local/bin/wu-plus"

    # Copy .desktop file
    escaped_dir=$(printf '%s\n' "$install_dir" | sed 's/[\\&|]/\\&/g')
    sed -e "s|^Icon=wu-plus$|Icon=$escaped_dir/share/icons/hicolor/512x512/apps/wu-plus.png|" \
        -e "s|^TryExec=wu-plus$|TryExec=$escaped_dir/bin/wu-plus|" \
        -e "s|^Exec=wu-plus |Exec=\"$escaped_dir/bin/wu-plus\" |" \
        "$install_dir/share/applications/$app_id.desktop" \
        > "$HOME/.local/share/applications/$app_id.desktop"
}

macos() {
    if [ -n "$bundle_path" ]; then
        cp "$bundle_path" "$temp/Wu-Plus-$arch.dmg"
    else
        echo "Downloading Wu Plus version: $version"
        curl "$download_base/Wu-Plus-$arch.dmg" > "$temp/Wu-Plus-$arch.dmg"
    fi
    # DiskImages cannot mount into temporary directories on some external volumes.
    mount_point="$(mktemp -d /tmp/wu-plus-mount.XXXXXX)"
    hdiutil attach -readonly -nobrowse "$temp/Wu-Plus-$arch.dmg" -mountpoint "$mount_point" >/dev/null
    dmg_mounted=true
    if [ ! -d "$mount_point/$mac_app" ]; then
        echo "The disk image does not contain $mac_app." >&2
        return 1
    fi
    echo "Installing $mac_app"
    if [ -d "/Applications/$mac_app" ]; then
        echo "Removing existing $mac_app"
        rm -rf "/Applications/$mac_app"
    fi
    ditto "$mount_point/$mac_app" "/Applications/$mac_app"
    hdiutil detach "$mount_point" >/dev/null
    dmg_mounted=false

    mkdir -p "$HOME/.local/bin"
    # Link the binary
    ln -sf "/Applications/$mac_app/Contents/MacOS/cli" "$HOME/.local/bin/wu-plus"
}

main "$@"
