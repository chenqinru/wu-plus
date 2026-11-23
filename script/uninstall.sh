#!/usr/bin/env sh
set -eu

# Uninstalls Wu Plus that was installed using the install.sh script.

remove_cli_link() {
    cli_link="$HOME/.local/bin/wu-plus"
    if [ -L "$cli_link" ] && [ "$(readlink "$cli_link")" = "$1" ]; then
        if [ -x "$2" ]; then
            ln -sf "$2" "$cli_link"
        else
            rm -f "$cli_link"
        fi
    fi
}

prompt_remove_preferences() {
    printf "Do you want to keep your Wu Plus user data and preferences? [Y/n] "
    IFS= read -r response || response=""
    case "$response" in
        [nN]|[nN][oO])
            rm -rf "$@"
            echo "Wu Plus user data and preferences removed."
            ;;
        *)
            echo "Wu Plus user data and preferences kept."
            ;;
    esac
}

linux() {
    stable_app="$HOME/.local/wu-plus.app"
    dev_app="$HOME/.local/wu-plus-dev.app"
    app="$stable_app"
    other_app="$dev_app"
    if [ "$channel" = "dev" ]; then
        app="$dev_app"
        other_app="$stable_app"
    fi

    remove_cli_link "$app/bin/wu-plus" "$other_app/bin/wu-plus"
    rm -rf "$app"
    rm -f "$HOME/.local/share/applications/$app_id.desktop"

    if [ ! -d "$stable_app" ] && [ ! -d "$dev_app" ]; then
        prompt_remove_preferences \
            "${XDG_CONFIG_HOME:-$HOME/.config}/wu plus" \
            "${XDG_DATA_HOME:-$HOME/.local/share}/wu plus" \
            "${XDG_STATE_HOME:-$HOME/.local/state}/wu plus" \
            "${XDG_CACHE_HOME:-$HOME/.cache}/wu plus"
    fi
}

macos() {
    stable_app="/Applications/Wu Plus.app"
    dev_app="/Applications/Wu Plus Dev.app"
    app="$stable_app"
    other_app="$dev_app"
    if [ "$channel" = "dev" ]; then
        app="$dev_app"
        other_app="$stable_app"
    fi

    remove_cli_link "$app/Contents/MacOS/cli" "$other_app/Contents/MacOS/cli"
    rm -rf "$app"

    if [ ! -d "$stable_app" ] && [ ! -d "$dev_app" ]; then
        prompt_remove_preferences "$HOME/Library/Application Support/Wu Plus"
    fi
}

main() {
    channel="${WU_CHANNEL:-${ZED_CHANNEL:-stable}}"
    case "$channel" in
        stable) app_id="io.github.chenqinru.wu-plus" ;;
        dev) app_id="io.github.chenqinru.wu-plus-dev" ;;
        *) echo "Unsupported release channel $channel" >&2; exit 1 ;;
    esac

    platform="$(uname -s)"
    case "$platform" in
        Darwin) macos ;;
        Linux) linux ;;
        *) echo "Unsupported platform $platform" >&2; exit 1 ;;
    esac
    echo "Wu Plus has been uninstalled"
}

main "$@"
