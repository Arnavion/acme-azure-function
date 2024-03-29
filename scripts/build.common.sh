#!/bin/bash

set -euo pipefail

if command -v gojq >/dev/null; then
    JQ='gojq'
else
    JQ='jq --sort-keys'
fi

target="$1"

func_name="$2"

azure_resource_group_name="$3"

azure_client_id="$4"
azure_client_secret="$5"

azure_storage_account_connection_string="$6"
azure_function_app_name="$7"

timer_trigger_schedule="$8"

secret_settings="$9"

case "$target" in
    'debug-http')
        binding="$(
            $JQ --null-input \
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
            $JQ --null-input \
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
            $JQ --null-input \
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

./scripts/containers.sh

secret_settings="$(
    $JQ --null-input --compact-output \
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
        $JQ --null-input --compact-output \
            --argjson SECRET_SETTINGS "$secret_settings" \
            --arg AZURE_CLIENT_ID "$azure_client_id" \
            --arg AZURE_CLIENT_SECRET "$azure_client_secret" \
            --arg AZURE_TENANT_ID "$(<<< "$AZURE_ACCOUNT" $JQ --raw-output '.tenantId')" \
            '$SECRET_SETTINGS + {
                "azure_client_id": $AZURE_CLIENT_ID,
                "azure_client_secret": $AZURE_CLIENT_SECRET,
                "azure_tenant_id": $AZURE_TENANT_ID,
            }'
    )"
fi

rm -rf ./dist
mkdir -p "./dist/$func_name"

case "$target" in
    'debug-http')
        extensions="$(
            $JQ --null-input \
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

>./dist/host.json $JQ --null-input \
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

>"./dist/$func_name/function.json" $JQ --null-input \
    --argjson binding "$binding" \
    '{
        "bindings": [$binding]
    }'

case "$target" in
    'debug-http')
        >./dist/local.settings.json $JQ --null-input \
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
        >./dist/local.settings.json $JQ --null-input \
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
        >./dist/local.settings.json $JQ --null-input \
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
        podman container run \
            --interactive \
            --rm \
            --tty \
            --userns=keep-id \
            "--volume=$PWD:$PWD" \
            "--volume=$(realpath ~/.cargo/git):$(realpath ~/.cargo/git)" \
            "--volume=$(realpath ~/.cargo/registry):$(realpath ~/.cargo/registry)" \
            "--workdir=$PWD" \
            localhost/azure-function-build-rust \
            make 'CARGOFLAGS=--target x86_64-unknown-linux-musl'
        cp -f ./target/x86_64-unknown-linux-musl/debug/acme-azure-function ./dist/main

        podman container run \
            --interactive \
            --rm \
            --tty \
            --publish=7071:7071 \
            --userns=keep-id \
            "--volume=$PWD:$PWD" \
            "--workdir=$PWD/dist/" \
            localhost/azure-function-build-func \
            func start -p 7071
        ;;

    'publish')
        podman container run \
            --interactive \
            --rm \
            --tty \
            --userns=keep-id \
            "--volume=$PWD:$PWD" \
            "--volume=$(realpath ~/.cargo/git):$(realpath ~/.cargo/git)" \
            "--volume=$(realpath ~/.cargo/registry):$(realpath ~/.cargo/registry)" \
            "--workdir=$PWD" \
            localhost/azure-function-build-rust \
            make 'CARGOFLAGS=--target x86_64-unknown-linux-musl --release'
        cp -f ./target/x86_64-unknown-linux-musl/release/acme-azure-function ./dist/main

        [ -d ~/.azure ]

        podman container run \
            --interactive \
            --rm \
            --tty \
            --userns=keep-id \
            "--volume=$PWD:$PWD" \
            "--volume=$HOME/.azure:$HOME/.azure" \
            "--workdir=$PWD" \
            localhost/azure-function-build-func \
            bash -c "
                set -euo pipefail

                az functionapp config appsettings set \
                    --resource-group '$azure_resource_group_name' --name '$azure_function_app_name' \
                    --settings 'SECRET_SETTINGS=$secret_settings'

                cd ./dist/

                func azure functionapp publish '$azure_function_app_name'
            "
        ;;
esac
