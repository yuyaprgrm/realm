#compdef realm

autoload -U is-at-least

_realm() {
    typeset -A opt_args
    typeset -a _arguments_options
    local ret=1

    if is-at-least 5.2; then
        _arguments_options=(-s -S -C)
    else
        _arguments_options=(-s -C)
    fi

    local context curcontext="$curcontext" state line
    _arguments "${_arguments_options[@]}" : \
'-c+[]:FILE:_files' \
'--config=[]:FILE:_files' \
'-h[Print help]' \
'--help[Print help]' \
":: :_realm_commands" \
"*::: :->realm" \
&& ret=0
    case $state in
    (realm)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:realm-command-$line[1]:"
        case $line[1] in
            (run)
_arguments "${_arguments_options[@]}" : \
'-c+[]:FILE:_files' \
'--config=[]:FILE:_files' \
'-h[Print help]' \
'--help[Print help]' \
':sandbox:(agent none-test private-test host-test)' \
'*::command:_default' \
&& ret=0
;;
(completion)
_arguments "${_arguments_options[@]}" : \
'-c+[]:FILE:_files' \
'--config=[]:FILE:_files' \
'-h[Print help]' \
'--help[Print help]' \
':shell:(bash elvish fish powershell zsh)' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_realm__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:realm-help-command-$line[1]:"
        case $line[1] in
            (run)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(completion)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
}

(( $+functions[_realm_commands] )) ||
_realm_commands() {
    local commands; commands=(
'run:Run a sandbox' \
'completion:Generate a shell completion script for the given shell' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'realm commands' commands "$@"
}
(( $+functions[_realm__subcmd__completion_commands] )) ||
_realm__subcmd__completion_commands() {
    local commands; commands=()
    _describe -t commands 'realm completion commands' commands "$@"
}
(( $+functions[_realm__subcmd__help_commands] )) ||
_realm__subcmd__help_commands() {
    local commands; commands=(
'run:Run a sandbox' \
'completion:Generate a shell completion script for the given shell' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'realm help commands' commands "$@"
}
(( $+functions[_realm__subcmd__help__subcmd__completion_commands] )) ||
_realm__subcmd__help__subcmd__completion_commands() {
    local commands; commands=()
    _describe -t commands 'realm help completion commands' commands "$@"
}
(( $+functions[_realm__subcmd__help__subcmd__help_commands] )) ||
_realm__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'realm help help commands' commands "$@"
}
(( $+functions[_realm__subcmd__help__subcmd__run_commands] )) ||
_realm__subcmd__help__subcmd__run_commands() {
    local commands; commands=()
    _describe -t commands 'realm help run commands' commands "$@"
}
(( $+functions[_realm__subcmd__run_commands] )) ||
_realm__subcmd__run_commands() {
    local commands; commands=()
    _describe -t commands 'realm run commands' commands "$@"
}

if [ "$funcstack[1]" = "_realm" ]; then
    _realm "$@"
else
    compdef _realm realm
fi
