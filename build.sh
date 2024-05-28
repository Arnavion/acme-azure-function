#!/bin/bash

set -euo pipefail

usage() {
    echo "Usage: $0 (debug | publish)"
}

target="${1:-}"

case "$target" in
    'debug'|'publish')
        ;;

    '--help')
        usage
        exit
        ;;

    *)
        usage >&2
        exit 1
        ;;
esac

./scripts/containers.sh

if command -v gojq >/dev/null; then
    JQ='gojq'
else
    JQ='jq --sort-keys'
fi

secret_settings="$(
    $JQ --null-input --compact-output \
        --arg ACME_CONTACT_URL "$ACME_CONTACT_URL" \
        --arg AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME "$AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME" \
        --arg AZURE_KEY_VAULT_CERTIFICATE_NAME "$AZURE_KEY_VAULT_CERTIFICATE_NAME" \
        --arg AZURE_KEY_VAULT_NAME "$AZURE_KEY_VAULT_NAME" \
        --arg AZURE_LOG_ANALYTICS_WORKSPACE_NAME "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
        --arg AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" \
        --arg AZURE_RESOURCE_GROUP_NAME "$AZURE_COMMON_RESOURCE_GROUP_NAME" \
        --arg AZURE_SUBSCRIPTION_ID "$AZURE_SUBSCRIPTION_ID" \
        --arg TOP_LEVEL_DOMAIN_NAME "$TOP_LEVEL_DOMAIN_NAME" \
        '{
            "acme_contact_url": $ACME_CONTACT_URL,
            "azure_key_vault_acme_account_key_name": $AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME,
            "azure_key_vault_acme_account_key_type": "ec:p384",
            "azure_key_vault_certificate_key_type": "rsa:4096:exportable",
            "azure_key_vault_certificate_name": $AZURE_KEY_VAULT_CERTIFICATE_NAME,
            "azure_key_vault_name": $AZURE_KEY_VAULT_NAME,
            "azure_log_analytics_workspace_name": $AZURE_LOG_ANALYTICS_WORKSPACE_NAME,
            "azure_log_analytics_workspace_resource_group_name": $AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME,
            "azure_resource_group_name": $AZURE_RESOURCE_GROUP_NAME,
            "azure_subscription_id": $AZURE_SUBSCRIPTION_ID,
            "top_level_domain_name": $TOP_LEVEL_DOMAIN_NAME
        }'
)"

func_name='renew-cert'

rm -rf ./dist
mkdir -p "./dist/$func_name"

case "$target" in
    'debug')
        >./dist/host.json $JQ --null-input \
            '{
                "version": "2.0",
                "customHandler": {
                    "description": {
                        "defaultExecutablePath": "main",
                    },
                },
                "extensions": {
                    "http": {
                        "dynamicThrottlesEnabled": false,
                        "maxConcurrentRequests": 1,
                        "maxOutstandingRequests": 1,
                        "routePrefix": "",
                    },
                },
            }'

        >"./dist/$func_name/function.json" $JQ --null-input \
            '{
                "bindings": [{
                    "name": "main",
                    "type": "httpTrigger",
                    "methods": ["Get"],
                    "authLevel": "function",
                }]
            }'

        secret_settings="$(
            $JQ --null-input --compact-output \
                --argjson SECRET_SETTINGS "$secret_settings" \
                --arg ACME_DIRECTORY_URL "$ACME_STAGING_DIRECTORY_URL" \
                --arg AZURE_CLIENT_ID "$AZURE_ACME_CLIENT_ID" \
                --arg AZURE_CLIENT_SECRET "$AZURE_ACME_CLIENT_SECRET" \
                --arg AZURE_TENANT_ID "$(<<< "$AZURE_ACCOUNT" $JQ --raw-output '.tenantId')" \
                '$SECRET_SETTINGS + {
                    "acme_directory_url": $ACME_DIRECTORY_URL,
                    "azure_client_id": $AZURE_CLIENT_ID,
                    "azure_client_secret": $AZURE_CLIENT_SECRET,
                    "azure_tenant_id": $AZURE_TENANT_ID,
                }'
        )"
        >./dist/local.settings.json $JQ --null-input \
            --arg SECRET_SETTINGS "$secret_settings" \
            '{
                "IsEncrypted": false,
                "Values": {
                    "FUNCTIONS_EXTENSION_VERSION": "~4",
                    "FUNCTIONS_WORKER_RUNTIME": "custom",
                    "SECRET_SETTINGS": $SECRET_SETTINGS
                }
            }'

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
        >./dist/host.json $JQ --null-input \
            '{
                "version": "2.0",
                "customHandler": {
                    "description": {
                        "defaultExecutablePath": "main",
                    },
                },
                "extensions": {},
            }'

        >"./dist/$func_name/function.json" $JQ --null-input \
            '{
                "bindings": [{
                    "name": "main",
                    "type": "timerTrigger",
                    "schedule": "0 17 1 * * *",
                    "runOnStartup": false,
                    "useMonitor": true,
                }]
            }'

        >./dist/local.settings.json $JQ --null-input \
            --arg AZURE_STORAGE_ACCOUNT_CONNECTION_STRING "$AZURE_ACME_STORAGE_ACCOUNT_CONNECTION_STRING" \
            '{
                "IsEncrypted": false,
                "Values": {
                    "AzureWebJobsStorage": $AZURE_STORAGE_ACCOUNT_CONNECTION_STRING,
                    "FUNCTIONS_EXTENSION_VERSION": "~4",
                    "FUNCTIONS_WORKER_RUNTIME": "custom"
                }
            }'

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

        secret_settings="$(
            $JQ --null-input --compact-output \
                --argjson SECRET_SETTINGS "$secret_settings" \
                --arg ACME_DIRECTORY_URL "$ACME_DIRECTORY_URL" \
                '$SECRET_SETTINGS + {
                    "acme_directory_url": $ACME_DIRECTORY_URL,
                }'
        )"

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
                    --resource-group '$AZURE_ACME_RESOURCE_GROUP_NAME' --name '$AZURE_ACME_FUNCTION_APP_NAME' \
                    --settings 'SECRET_SETTINGS=$secret_settings'

                cd ./dist/

                func azure functionapp publish '$AZURE_ACME_FUNCTION_APP_NAME'
            "
        ;;
esac
