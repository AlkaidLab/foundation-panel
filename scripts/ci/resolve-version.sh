#!/usr/bin/env bash
# Resolve the single version one workflow run uses for both the injected build
# version and, when it publishes, the release tag (panel-gha-dispatch-version).
#
# A tag push uses the tag's version, a manual run that publishes uses the UTC
# date form already used by the release tags, and every other run keeps the
# development version.
set -euo pipefail

event=""
ref=""
create_release=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --event)
            event="${2:?--event needs a value}"
            shift 2
            ;;
        --ref)
            ref="${2:?--ref needs a value}"
            shift 2
            ;;
        --create-release)
            create_release="${2-}"
            shift
            if [ "$#" -gt 0 ]; then
                shift
            fi
            ;;
        *)
            echo "resolve-version: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

if [[ "$ref" == refs/tags/v* ]]; then
    printf '%s\n' "${ref#refs/tags/v}"
elif [ "$event" = "workflow_dispatch" ] && [ "$create_release" = "true" ]; then
    printf '%s\n' "$(date -u +%Y.%m%d.%H%M%S)"
else
    printf '%s\n' "0.0.0-dev"
fi
