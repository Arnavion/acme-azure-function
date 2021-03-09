#!/bin/bash

set -euo pipefail

target="$1"

func_name="$2"
func_dir_name="$3"

azure_resource_group_name="$4"

azure_client_id="$5"
azure_client_secret="$6"

azure_storage_account_connection_string="$7"
azure_function_app_name="$8"

timer_trigger_schedule="$9"

secret_settings="${10}"

case "$target" in
    'debug-http')
        binding="$(
            jq --null-input --sort-keys \
                '{
                    "name": "main",
                    "type": "httpTrigger",
                    "methods": ["Get"],
                    "authLevel": "function",
                }'
        )"
        ;;

    'debug-timer')
        binding="$(
            jq --null-input --sort-keys \
                '{
                    "name": "main",
                    "type": "timerTrigger",
                    "schedule": "* * * * * *",
                    "runOnStartup": false,
                    "useMonitor": true,
                }'
        )"
        ;;

    'publish')
        binding="$(
            jq --null-input --sort-keys \
                --arg schedule "$timer_trigger_schedule" \
                '{
                    "name": "main",
                    "type": "timerTrigger",
                    "schedule": $schedule,
                    "runOnStartup": false,
                    "useMonitor": true,
                }'
        )"
        ;;
esac

./scripts/docker.sh

secret_settings="$(
    jq --null-input --sort-keys --compact-output \
        --argjson SECRET_SETTINGS "$secret_settings" \
        --arg AZURE_SUBSCRIPTION_ID "$AZURE_SUBSCRIPTION_ID" \
        --arg AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" \
        --arg AZURE_LOG_ANALYTICS_WORKSPACE_NAME "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
        '$SECRET_SETTINGS + {
            "azure_subscription_id": $AZURE_SUBSCRIPTION_ID,
            "azure_log_analytics_workspace_resource_group_name": $AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME,
            "azure_log_analytics_workspace_name": $AZURE_LOG_ANALYTICS_WORKSPACE_NAME,
        }'
)"

if [[ "$target" == debug* ]]; then
    secret_settings="$(
        jq --null-input --sort-keys --compact-output \
            --argjson SECRET_SETTINGS "$secret_settings" \
            --arg AZURE_CLIENT_ID "$azure_client_id" \
            --arg AZURE_CLIENT_SECRET "$azure_client_secret" \
            --arg AZURE_TENANT_ID "$(echo "$AZURE_ACCOUNT" | jq --raw-output '.tenantId')" \
            '$SECRET_SETTINGS + {
                "azure_client_id": $AZURE_CLIENT_ID,
                "azure_client_secret": $AZURE_CLIENT_SECRET,
                "azure_tenant_id": $AZURE_TENANT_ID,
            }'
    )"
fi

rm -rf "./$func_dir_name/dist"
mkdir -p "./$func_dir_name/dist/$func_name"

case "$target" in
    'debug-http')
        extensions="$(
            jq --null-input --sort-keys \
                '{
                    "http": {
                        "dynamicThrottlesEnabled": false,
                        "maxConcurrentRequests": 1,
                        "maxOutstandingRequests": 1,
                        "routePrefix": "",
                    },
                }'
        )"
        ;;

    'debug-timer'|'publish')
        extensions='{}'
        ;;
esac

>"./$func_dir_name/dist/host.json" jq --null-input --sort-keys \
    --argjson 'extensions' "$extensions" \
    '{
        "version": "2.0",
        "customHandler": {
            "description": {
                "defaultExecutablePath": "main",
            },
        },
        "extensions": $extensions,
    }'

>"./$func_dir_name/dist/$func_name/function.json" jq --null-input --sort-keys \
    --argjson binding "$binding" \
    '{
        "bindings": [$binding]
    }'

case "$target" in
    'debug-http')
        >"./$func_dir_name/dist/local.settings.json" jq --null-input --sort-keys \
            --arg SECRET_SETTINGS "$secret_settings" \
            '{
                "IsEncrypted": false,
                "Values": {
                    "FUNCTIONS_WORKER_RUNTIME": "Custom",
                    "SECRET_SETTINGS": $SECRET_SETTINGS
                }
            }'
        ;;

    'debug-timer')
        >"./$func_dir_name/dist/local.settings.json" jq --null-input --sort-keys \
            --arg AZURE_STORAGE_ACCOUNT_CONNECTION_STRING "$azure_storage_account_connection_string" \
            --arg SECRET_SETTINGS "$secret_settings" \
            '{
                "IsEncrypted": false,
                "Values": {
                    "AzureWebJobsStorage": $AZURE_STORAGE_ACCOUNT_CONNECTION_STRING,
                    "FUNCTIONS_WORKER_RUNTIME": "Custom",
                    "SECRET_SETTINGS": $SECRET_SETTINGS
                }
            }'
        ;;

    'publish')
        >"./$func_dir_name/dist/local.settings.json" jq --null-input --sort-keys \
            --arg AZURE_STORAGE_ACCOUNT_CONNECTION_STRING "$azure_storage_account_connection_string" \
            '{
                "IsEncrypted": false,
                "Values": {
                    "AzureWebJobsStorage": $AZURE_STORAGE_ACCOUNT_CONNECTION_STRING,
                    "FUNCTIONS_WORKER_RUNTIME": "Custom"
                }
            }'
        ;;
esac

case "$target" in
    debug-*)
        docker run \
            -it \
            --rm \
            -v "$PWD:$PWD" \
            -v "$(realpath ~/.cargo/git):$(realpath ~/.cargo/git)" \
            -v "$(realpath ~/.cargo/registry):$(realpath ~/.cargo/registry)" \
            -u "$(id -u)" \
            -w "$PWD" \
            'azure-function-build-rust' \
            sh -c 'make CARGOFLAGS="--target x86_64-unknown-linux-musl"'
        cp -f "./target/x86_64-unknown-linux-musl/debug/$func_dir_name" "./$func_dir_name/dist/main"

        docker run \
            -it \
            --rm \
            -p '7071:7071' \
            -v "$PWD:$PWD" \
            -u "$(id -u)" \
            -w "$PWD" \
            'azure-function-build-func' \
            sh -c "cd './$func_dir_name/dist/' && func start -p 7071"
        ;;

    'publish')
        docker run \
            -it \
            --rm \
            -v "$PWD:$PWD" \
            -v "$(realpath ~/.cargo/git):$(realpath ~/.cargo/git)" \
            -v "$(realpath ~/.cargo/registry):$(realpath ~/.cargo/registry)" \
            -u "$(id -u)" \
            -w "$PWD" \
            'azure-function-build-rust' \
            sh -c 'make CARGOFLAGS="--target x86_64-unknown-linux-musl --release"'
        cp -f "./target/x86_64-unknown-linux-musl/release/$func_dir_name" "./$func_dir_name/dist/main"

        [ -d ~/.azure ]

        docker run \
            -it \
            --rm \
            -v "$PWD:$PWD" \
            -v "$HOME/.azure:$HOME/.azure" \
            -u "$(id -u)" \
            -w "$PWD" \
            'azure-function-build-func' \
            bash -c "
                set -euo pipefail

                az functionapp config appsettings set \
                    --resource-group '$azure_resource_group_name' --name '$azure_function_app_name' \
                    --settings 'SECRET_SETTINGS=$secret_settings'

                cd './$func_dir_name/dist/'

                func azure functionapp publish '$azure_function_app_name'
            "
        ;;
esac
