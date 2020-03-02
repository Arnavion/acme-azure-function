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
        echo "Usage: ./UpdateCdnCertificateFunction/build.sh (debug|release|publish)" >&2
        exit 1
        ;;
esac

./docker/build.sh

secret_settings="$(
    jq --null-input --sort-keys --compact-output \
        --arg AZURE_SUBSCRIPTION_ID "$AZURE_SUBSCRIPTION_ID" \
        --arg AZURE_RESOURCE_GROUP_NAME "$AZURE_RESOURCE_GROUP_NAME" \
        --arg AZURE_CDN_PROFILE_NAME "$AZURE_CDN_PROFILE_NAME" \
        --arg AZURE_CDN_ENDPOINT_NAME "$AZURE_CDN_ENDPOINT_NAME" \
        --arg AZURE_CDN_CUSTOM_DOMAIN_NAME "${DOMAIN_NAME//./-}" \
        --arg AZURE_KEYVAULT_NAME "$AZURE_KEYVAULT_NAME" \
        --arg AZURE_KEYVAULT_CERTIFICATE_NAME "$AZURE_KEYVAULT_CERTIFICATE_NAME" \
        --arg AZURE_STORAGE_ACCOUNT_NAME "$AZURE_CDN_STORAGE_ACCOUNT_NAME" \
        '{
            "AzureSubscriptionId": $AZURE_SUBSCRIPTION_ID,
            "AzureResourceGroupName": $AZURE_RESOURCE_GROUP_NAME,
            "AzureCdnProfileName": $AZURE_CDN_PROFILE_NAME,
            "AzureCdnEndpointName": $AZURE_CDN_ENDPOINT_NAME,
            "AzureCdnCustomDomainName": $AZURE_CDN_CUSTOM_DOMAIN_NAME,
            "AzureKeyVaultName": $AZURE_KEYVAULT_NAME,
            "AzureKeyVaultCertificateName": $AZURE_KEYVAULT_CERTIFICATE_NAME,
            "AzureStorageAccountName": $AZURE_STORAGE_ACCOUNT_NAME,
        }'
)"
if [ "$local" = 'true' ]; then
    secret_settings="$(
        jq --null-input --sort-keys --compact-output \
            --arg SECRET_SETTINGS "$secret_settings" \
            --arg AZURE_CLIENT_ID "$AZURE_CDN_CLIENT_ID" \
            --arg AZURE_CLIENT_SECRET "$AZURE_CDN_CLIENT_SECRET" \
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
        >./UpdateCdnCertificateFunction/local.settings.json jq --null-input --sort-keys \
            --arg AZURE_STORAGE_ACCOUNT_CONNECTION_STRING "$AZURE_CDN_STORAGE_ACCOUNT_CONNECTION_STRING" \
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
                cd ./UpdateCdnCertificateFunction/ &&
                dotnet publish --nologo -c '$configuration' '-p:LOCAL=$local' &&
                if [ '$local' = 'true' ]; then
                    cd ./bin/$configuration/netcoreapp3.1/ && func start -p 7071
                else
                    exit 1
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
                    --resource-group '$AZURE_RESOURCE_GROUP_NAME' --name '$AZURE_CDN_FUNCTION_APP_NAME' \
                    --settings 'SECRET_SETTINGS=$secret_settings' &&

                cd ./UpdateCdnCertificateFunction/bin/$configuration/netcoreapp3.1/publish/ &&

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

                func azure functionapp publish '$AZURE_CDN_FUNCTION_APP_NAME'
            "
        ;;
esac
