_bhtune() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="bhtune"
                ;;
            bhtune,export)
                cmd="bhtune__subcmd__export"
                ;;
            bhtune,help)
                cmd="bhtune__subcmd__help"
                ;;
            bhtune,history)
                cmd="bhtune__subcmd__history"
                ;;
            bhtune,opc)
                cmd="bhtune__subcmd__opc"
                ;;
            bhtune,simulate)
                cmd="bhtune__subcmd__simulate"
                ;;
            bhtune,template)
                cmd="bhtune__subcmd__template"
                ;;
            bhtune,tune)
                cmd="bhtune__subcmd__tune"
                ;;
            bhtune__subcmd__help,export)
                cmd="bhtune__subcmd__help__subcmd__export"
                ;;
            bhtune__subcmd__help,help)
                cmd="bhtune__subcmd__help__subcmd__help"
                ;;
            bhtune__subcmd__help,history)
                cmd="bhtune__subcmd__help__subcmd__history"
                ;;
            bhtune__subcmd__help,opc)
                cmd="bhtune__subcmd__help__subcmd__opc"
                ;;
            bhtune__subcmd__help,simulate)
                cmd="bhtune__subcmd__help__subcmd__simulate"
                ;;
            bhtune__subcmd__help,template)
                cmd="bhtune__subcmd__help__subcmd__template"
                ;;
            bhtune__subcmd__help,tune)
                cmd="bhtune__subcmd__help__subcmd__tune"
                ;;
            bhtune__subcmd__help__subcmd__history,list)
                cmd="bhtune__subcmd__help__subcmd__history__subcmd__list"
                ;;
            bhtune__subcmd__help__subcmd__history,prune)
                cmd="bhtune__subcmd__help__subcmd__history__subcmd__prune"
                ;;
            bhtune__subcmd__help__subcmd__history,revert)
                cmd="bhtune__subcmd__help__subcmd__history__subcmd__revert"
                ;;
            bhtune__subcmd__help__subcmd__history,show)
                cmd="bhtune__subcmd__help__subcmd__history__subcmd__show"
                ;;
            bhtune__subcmd__help__subcmd__opc,browse)
                cmd="bhtune__subcmd__help__subcmd__opc__subcmd__browse"
                ;;
            bhtune__subcmd__help__subcmd__opc,read)
                cmd="bhtune__subcmd__help__subcmd__opc__subcmd__read"
                ;;
            bhtune__subcmd__help__subcmd__opc,servers)
                cmd="bhtune__subcmd__help__subcmd__opc__subcmd__servers"
                ;;
            bhtune__subcmd__help__subcmd__opc,write)
                cmd="bhtune__subcmd__help__subcmd__opc__subcmd__write"
                ;;
            bhtune__subcmd__help__subcmd__template,delete)
                cmd="bhtune__subcmd__help__subcmd__template__subcmd__delete"
                ;;
            bhtune__subcmd__help__subcmd__template,export)
                cmd="bhtune__subcmd__help__subcmd__template__subcmd__export"
                ;;
            bhtune__subcmd__help__subcmd__template,import)
                cmd="bhtune__subcmd__help__subcmd__template__subcmd__import"
                ;;
            bhtune__subcmd__help__subcmd__template,list)
                cmd="bhtune__subcmd__help__subcmd__template__subcmd__list"
                ;;
            bhtune__subcmd__help__subcmd__template,show)
                cmd="bhtune__subcmd__help__subcmd__template__subcmd__show"
                ;;
            bhtune__subcmd__history,help)
                cmd="bhtune__subcmd__history__subcmd__help"
                ;;
            bhtune__subcmd__history,list)
                cmd="bhtune__subcmd__history__subcmd__list"
                ;;
            bhtune__subcmd__history,prune)
                cmd="bhtune__subcmd__history__subcmd__prune"
                ;;
            bhtune__subcmd__history,revert)
                cmd="bhtune__subcmd__history__subcmd__revert"
                ;;
            bhtune__subcmd__history,show)
                cmd="bhtune__subcmd__history__subcmd__show"
                ;;
            bhtune__subcmd__history__subcmd__help,help)
                cmd="bhtune__subcmd__history__subcmd__help__subcmd__help"
                ;;
            bhtune__subcmd__history__subcmd__help,list)
                cmd="bhtune__subcmd__history__subcmd__help__subcmd__list"
                ;;
            bhtune__subcmd__history__subcmd__help,prune)
                cmd="bhtune__subcmd__history__subcmd__help__subcmd__prune"
                ;;
            bhtune__subcmd__history__subcmd__help,revert)
                cmd="bhtune__subcmd__history__subcmd__help__subcmd__revert"
                ;;
            bhtune__subcmd__history__subcmd__help,show)
                cmd="bhtune__subcmd__history__subcmd__help__subcmd__show"
                ;;
            bhtune__subcmd__opc,browse)
                cmd="bhtune__subcmd__opc__subcmd__browse"
                ;;
            bhtune__subcmd__opc,help)
                cmd="bhtune__subcmd__opc__subcmd__help"
                ;;
            bhtune__subcmd__opc,read)
                cmd="bhtune__subcmd__opc__subcmd__read"
                ;;
            bhtune__subcmd__opc,servers)
                cmd="bhtune__subcmd__opc__subcmd__servers"
                ;;
            bhtune__subcmd__opc,write)
                cmd="bhtune__subcmd__opc__subcmd__write"
                ;;
            bhtune__subcmd__opc__subcmd__help,browse)
                cmd="bhtune__subcmd__opc__subcmd__help__subcmd__browse"
                ;;
            bhtune__subcmd__opc__subcmd__help,help)
                cmd="bhtune__subcmd__opc__subcmd__help__subcmd__help"
                ;;
            bhtune__subcmd__opc__subcmd__help,read)
                cmd="bhtune__subcmd__opc__subcmd__help__subcmd__read"
                ;;
            bhtune__subcmd__opc__subcmd__help,servers)
                cmd="bhtune__subcmd__opc__subcmd__help__subcmd__servers"
                ;;
            bhtune__subcmd__opc__subcmd__help,write)
                cmd="bhtune__subcmd__opc__subcmd__help__subcmd__write"
                ;;
            bhtune__subcmd__template,delete)
                cmd="bhtune__subcmd__template__subcmd__delete"
                ;;
            bhtune__subcmd__template,export)
                cmd="bhtune__subcmd__template__subcmd__export"
                ;;
            bhtune__subcmd__template,help)
                cmd="bhtune__subcmd__template__subcmd__help"
                ;;
            bhtune__subcmd__template,import)
                cmd="bhtune__subcmd__template__subcmd__import"
                ;;
            bhtune__subcmd__template,list)
                cmd="bhtune__subcmd__template__subcmd__list"
                ;;
            bhtune__subcmd__template,show)
                cmd="bhtune__subcmd__template__subcmd__show"
                ;;
            bhtune__subcmd__template__subcmd__help,delete)
                cmd="bhtune__subcmd__template__subcmd__help__subcmd__delete"
                ;;
            bhtune__subcmd__template__subcmd__help,export)
                cmd="bhtune__subcmd__template__subcmd__help__subcmd__export"
                ;;
            bhtune__subcmd__template__subcmd__help,help)
                cmd="bhtune__subcmd__template__subcmd__help__subcmd__help"
                ;;
            bhtune__subcmd__template__subcmd__help,import)
                cmd="bhtune__subcmd__template__subcmd__help__subcmd__import"
                ;;
            bhtune__subcmd__template__subcmd__help,list)
                cmd="bhtune__subcmd__template__subcmd__help__subcmd__list"
                ;;
            bhtune__subcmd__template__subcmd__help,show)
                cmd="bhtune__subcmd__template__subcmd__help__subcmd__show"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        bhtune)
            opts="-h -V --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help --version tune simulate template history export opc help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__export)
            opts="-h --format --output --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --format)
                    COMPREPLY=($(compgen -W "csv json" -- "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help)
            opts="tune simulate template history export opc help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__export)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__history)
            opts="list show revert prune"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__history__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__history__subcmd__prune)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__history__subcmd__revert)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__history__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__opc)
            opts="servers read write browse"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__opc__subcmd__browse)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__opc__subcmd__read)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__opc__subcmd__servers)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__opc__subcmd__write)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__simulate)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__template)
            opts="list show import export delete"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__template__subcmd__delete)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__template__subcmd__export)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__template__subcmd__import)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__template__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__template__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__help__subcmd__tune)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history)
            opts="-h --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help list show revert prune help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__help)
            opts="list show revert prune help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__help__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__help__subcmd__prune)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__help__subcmd__revert)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__list)
            opts="-h --outcome --limit --offset --output --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --outcome)
                    COMPREPLY=($(compgen -W "running completed failed aborted" -- "${cur}"))
                    return 0
                    ;;
                --limit)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --offset)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "table json" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__prune)
            opts="-h --older-than-days --dry-run --output --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --older-than-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "table json" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__revert)
            opts="-h --bridge-host --server --yes --output --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --bridge-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "table json" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__history__subcmd__show)
            opts="-h --output --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --output)
                    COMPREPLY=($(compgen -W "table json" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc)
            opts="-h --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help servers read write browse help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__browse)
            opts="-h --bridge-host --server --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --bridge-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__help)
            opts="servers read write browse help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__help__subcmd__browse)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__help__subcmd__read)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__help__subcmd__servers)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__help__subcmd__write)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__read)
            opts="-h --bridge-host --server --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --bridge-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__servers)
            opts="-h --bridge-host --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --bridge-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__opc__subcmd__write)
            opts="-h --bridge-host --server --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --bridge-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__simulate)
            opts="-t -h --tagname --template --process-type --controller-type --relay-amp --cycles-skip --cycles-count --noise-protection-secs --sim-gain --sim-tau --sim-dead-time --sim-noise --sim-seed --sim-initial-pv --sim-initial-mv --notes --yes --write-pid --output --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --tagname)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -t)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --template)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --process-type)
                    COMPREPLY=($(compgen -W "flow pressure-line pressure-vessel level temperature-mixing temperature-heat-exchange" -- "${cur}"))
                    return 0
                    ;;
                --controller-type)
                    COMPREPLY=($(compgen -W "p pi pid" -- "${cur}"))
                    return 0
                    ;;
                --relay-amp)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cycles-skip)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cycles-count)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --noise-protection-secs)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-gain)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-tau)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-dead-time)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-noise)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-seed)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-initial-pv)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-initial-mv)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --notes)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --write-pid)
                    COMPREPLY=($(compgen -W "aggressive moderate sluggish" -- "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "table json" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template)
            opts="-h --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help list show import export delete help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__delete)
            opts="-h --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__export)
            opts="-h --format --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --format)
                    COMPREPLY=($(compgen -W "json toml" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__help)
            opts="list show import export delete help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__help__subcmd__delete)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__help__subcmd__export)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__help__subcmd__import)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__help__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__import)
            opts="-h --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__list)
            opts="-h --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__template__subcmd__show)
            opts="-h --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        bhtune__subcmd__tune)
            opts="-t -h --tagname --template --process-type --controller-type --relay-amp --cycles-skip --cycles-count --noise-protection-secs --driver --bridge-host --server --sim-gain --sim-tau --sim-dead-time --sim-noise --sim-seed --sim-initial-pv --sim-initial-mv --pv-range-high --pv-range-low --mv-range-high --mv-range-low --direction --notes --yes --write-pid --output --config --db --templates --retention-days --log-level --log-dir --log-format --log-rotation --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --tagname)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -t)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --template)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --process-type)
                    COMPREPLY=($(compgen -W "flow pressure-line pressure-vessel level temperature-mixing temperature-heat-exchange" -- "${cur}"))
                    return 0
                    ;;
                --controller-type)
                    COMPREPLY=($(compgen -W "p pi pid" -- "${cur}"))
                    return 0
                    ;;
                --relay-amp)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cycles-skip)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cycles-count)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --noise-protection-secs)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --driver)
                    COMPREPLY=($(compgen -W "opcda simulator" -- "${cur}"))
                    return 0
                    ;;
                --bridge-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-gain)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-tau)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-dead-time)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-noise)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-seed)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-initial-pv)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sim-initial-mv)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pv-range-high)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pv-range-low)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --mv-range-high)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --mv-range-low)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --direction)
                    COMPREPLY=($(compgen -W "direct reverse" -- "${cur}"))
                    return 0
                    ;;
                --notes)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --write-pid)
                    COMPREPLY=($(compgen -W "aggressive moderate sluggish" -- "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "table json" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --templates)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --retention-days)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-format)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-rotation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _bhtune -o nosort -o bashdefault -o default bhtune
else
    complete -F _bhtune -o bashdefault -o default bhtune
fi
