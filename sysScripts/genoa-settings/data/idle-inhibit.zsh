if (( $+commands[genoa-idle] )); then
    _genoa_build_with_suspend_protection() {
        command genoa-idle inhibit-suspend building "$@"
    }

    cargo() {
        case "$1" in
            bench|build|check|clippy|doc|fix|install|run|test)
                _genoa_build_with_suspend_protection cargo "$@"
                ;;
            *)
                command cargo "$@"
                ;;
        esac
    }

    make() {
        _genoa_build_with_suspend_protection make "$@"
    }

    ninja() {
        _genoa_build_with_suspend_protection ninja "$@"
    }

    just() {
        _genoa_build_with_suspend_protection just "$@"
    }

    cmake() {
        if [[ "$1" == "--build" ]]; then
            _genoa_build_with_suspend_protection cmake "$@"
        else
            command cmake "$@"
        fi
    }
fi
