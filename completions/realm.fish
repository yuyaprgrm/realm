# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_realm_global_optspecs
    string join \n c/config= h/help
end

function __fish_realm_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_realm_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_realm_using_subcommand
    set -l cmd (__fish_realm_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c realm -n "__fish_realm_needs_command" -s c -l config -r -F
complete -c realm -n "__fish_realm_needs_command" -s h -l help -d 'Print help'
complete -c realm -n "__fish_realm_needs_command" -f -a "run" -d 'Run a sandbox'
complete -c realm -n "__fish_realm_needs_command" -f -a "completion" -d 'Generate a shell completion script for the given shell'
complete -c realm -n "__fish_realm_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c realm -n "__fish_realm_using_subcommand run" -s c -l config -r -F
complete -c realm -n "__fish_realm_using_subcommand run" -s h -l help -d 'Print help'
complete -c realm -n "__fish_realm_using_subcommand completion" -s c -l config -r -F
complete -c realm -n "__fish_realm_using_subcommand completion" -s h -l help -d 'Print help'
complete -c realm -n "__fish_realm_using_subcommand help; and not __fish_seen_subcommand_from run completion help" -f -a "run" -d 'Run a sandbox'
complete -c realm -n "__fish_realm_using_subcommand help; and not __fish_seen_subcommand_from run completion help" -f -a "completion" -d 'Generate a shell completion script for the given shell'
complete -c realm -n "__fish_realm_using_subcommand help; and not __fish_seen_subcommand_from run completion help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c realm -n "__fish_realm_using_subcommand run" -f -a "agent none-test private-test host-test"
