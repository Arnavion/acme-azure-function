#!/bin/bash

set -euo pipefail

case "${1:-}" in
    'debug')
        configuration='Debug'
        local='true'
        acme_directory_url="$ACME_STAGING_DIRECTORY_URL"
        ;;

    'release')
        configuration='Release'
        local='false'
        acme_directory_url="$ACME_DIRECTORY_URL"
        ;;

    'publish')
        configuration='Release'
        local='false'
        acme_directory_url="$ACME_DIRECTORY_URL"
        ;;

    *)
        echo "Usage: ./build.sh (debug|release|publish)" >&2
        exit 1
        ;;
esac

./docker/build.sh

secret_settings="$(
    jq --null-input --sort-keys --compact-output \
        --arg ACME_DIRECTORY_URL "$acme_directory_url" \
        --arg ACME_ACCOUNT_KEY_SECRET_NAME "$ACME_ACCOUNT_KEY_SECRET_NAME" \
        --arg ACME_CONTACT_URL "$ACME_CONTACT_URL" \
        --arg AZURE_SUBSCRIPTION_ID "$AZURE_SUBSCRIPTION_ID" \
        --arg AZURE_RESOURCE_GROUP_NAME "$AZURE_RESOURCE_GROUP_NAME" \
        --arg AZURE_KEYVAULT_NAME "$AZURE_KEYVAULT_NAME" \
        --arg AZURE_KEYVAULT_CERTIFICATE_NAME "$AZURE_KEYVAULT_CERTIFICATE_NAME" \
        --arg TOP_LEVEL_DOMAIN_NAME "$TOP_LEVEL_DOMAIN_NAME" \
        '{
            "AcmeDirectoryURL": $ACME_DIRECTORY_URL,
            "AcmeAccountKeySecretName": $ACME_ACCOUNT_KEY_SECRET_NAME,
            "AcmeContactURL": $ACME_CONTACT_URL,
            "AzureSubscriptionId": $AZURE_SUBSCRIPTION_ID,
            "AzureResourceGroupName": $AZURE_RESOURCE_GROUP_NAME,
            "AzureKeyVaultName": $AZURE_KEYVAULT_NAME,
            "AzureKeyVaultCertificateName": $AZURE_KEYVAULT_CERTIFICATE_NAME,
            "TopLevelDomainName": $TOP_LEVEL_DOMAIN_NAME
        }'
)"
if [ "$local" = 'true' ]; then
    secret_settings="$(
        jq --null-input --sort-keys --compact-output \
            --arg SECRET_SETTINGS "$secret_settings" \
            --arg AZURE_CLIENT_ID "$AZURE_ACME_CLIENT_ID" \
            --arg AZURE_CLIENT_SECRET "$AZURE_ACME_CLIENT_SECRET" \
            --arg AZURE_TENANT_ID "$(echo "$AZURE_ACCOUNT" | jq --raw-output '.tenantId')" \
            '
                ($SECRET_SETTINGS | fromjson) + {
                    "AzureClientID": $AZURE_CLIENT_ID,
                    "AzureClientSecret": $AZURE_CLIENT_SECRET,
                    "AzureTenantID": $AZURE_TENANT_ID,
                }
            '
    )"
fi

case "$1" in
    debug|release)
        >./AcmeFunction/local.settings.json jq --null-input --sort-keys \
            --arg AZURE_STORAGE_ACCOUNT_CONNECTION_STRING "$AZURE_ACME_STORAGE_ACCOUNT_CONNECTION_STRING" \
            --arg SECRET_SETTINGS "$secret_settings" \
            '{
                "IsEncrypted": false,
                "Values": {
                    "AzureWebJobsStorage": $AZURE_STORAGE_ACCOUNT_CONNECTION_STRING,
                    "FUNCTIONS_EXTENSION_VERSION": "~3",
                    "FUNCTIONS_WORKER_RUNTIME": "dotnet",
                    "SECRET_SETTINGS": $SECRET_SETTINGS
                }
            }'

        mkdir -p ~/.nuget

        docker run \
            -it \
            --rm \
            -p '7071:7071' \
            -v "$PWD:$PWD" \
            -v "$HOME/.nuget:$HOME/.nuget" \
            -w "$PWD" \
            'acme-azure-function-build' \
            sh -c "
                mkdir -p ~/.dotnet/ &&
                touch ~/.dotnet/\"\$(dotnet --version)\".dotnetFirstUseSentinel &&
                cd ./AcmeFunction/ &&
                dotnet publish --nologo -c '$configuration' '-p:LOCAL=$local' &&
                if [ '$local' = 'true' ]; then
                    cd ./bin/$configuration/netcoreapp3.1/ && func start -p 7071
                fi
            "
        ;;

    publish)
        [ -d ~/.azure ]

        docker run \
            -it \
            --rm \
            -v "$PWD:$PWD" \
            -v "$HOME/.azure:$HOME/.azure" \
            -w "$PWD" \
            'acme-azure-function-build' \
            sh -c "
                az functionapp config appsettings set \
                    --resource-group '$AZURE_RESOURCE_GROUP_NAME' --name '$AZURE_ACME_FUNCTION_APP_NAME' \
                    --settings 'SECRET_SETTINGS=$secret_settings' &&

                cd ./AcmeFunction/bin/$configuration/netcoreapp3.1/publish/ &&

                >./host.json jq --null-input --sort-keys \
                    '{
                        \"version\": \"2.0\",
                        \"logging\": {
                            \"applicationInsights\": {
                                \"samplingSettings\": {
                                    \"isEnabled\": false
                                }
                            }
                        }
                    }' &&

                >./local.settings.json echo '{ \"IsEncrypted\": false, \"Values\": { \"FUNCTIONS_EXTENSION_VERSION\": \"~3\", \"FUNCTIONS_WORKER_RUNTIME\": \"dotnet\" } }' &&

                func azure functionapp publish '$AZURE_ACME_FUNCTION_APP_NAME'
            "
        ;;
esac
