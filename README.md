This repository contains Azure Functions that can be used to provision TLS certificates for a static website hosted on Azure Storage and served by Azure CDN.

The certificates are provisioned using [the ACME v2 protocol.](https://ietf-wg-acme.github.io/acme/draft-ietf-acme-acme.html)

It is used for <https://www.arnavion.dev>, with Let's Encrypt as the ACME server.


# Azure setup

- An Azure storage account. The storage account is enabled to serve static websites, which automatically creates a container named `$web`. Any block blobs placed in this container are served by the storage account's web endpoint.

    The `$web` container has an access level of "Blob (anonymous read access for blobs only)"

- An Azure CDN profile and endpoint. The endpoint has a "Custom origin" pointing to the storage account's web endpoint.

    Note that the CDN endpoint's origin type is *not* set to "Storage", since this would require the container name to be present in the URL path. Eg `https://cdnendpoint.azureedge.net/$web/index.html` instead of `https://cdnendpoint.azureedge.net/index.html`

- A custom domain on the CDN endpoint.

    Note that Azure DNS does *not* need to be used for the custom domain.

- An Azure KeyVault to hold the ACME account key and the TLS certificate for the custom domain.

- An Azure Function app.

- An Azure Service Principal used by the functions to access to the above-mentioned Azure resources. See below for the precise permissions this SP needs.


There are two "entrypoint" functions, both using timer triggers:

- `RenewKeyVaultCertificateOrchestratorManager`

    This function is a Durable Function orchestrator manager. It spawns an instance of `RenewKeyVaultCertificateOrchestratorInstance` that is a Durable Function orchestrator.

    The orchestrator uses multiple sub-functions to achieve the task of renewing the certificate in the KeyVault. It checks the expiry of the certificate in the KeyVault. If the certificate needs to be renewed, it then uses the ACME v2 protocol to get a new certificate, and uploads this new certificate to the KeyVault.

- `UpdateCdnCertificate`

    This function compares the certificate in the KeyVault with the certificate set in the CDN custom domain. If the two do not match, it updates the custom domain to use the certificate in the KeyVault.

The reason to have two separate functions is to allow the CDN custom domain to use the latest certificate *regardless* of how the certificate was created.


```sh
# See `Settings.fs` for an explanation of what each variable means.

DOMAIN_NAME='...'

AZURE_SP_NAME='http://...'

AZURE_RESOURCE_GROUP_NAME='...'

AZURE_APP_INSIGHTS_NAME='...'

AZURE_CDN_PROFILE_NAME='...'
AZURE_CDN_ENDPOINT_NAME='...'

AZURE_KEYVAULT_NAME='...'

# Must be unique for each function app. Multiple function apps cannot share the same storage account because of
# https://github.com/Azure/azure-functions-host/issues/4499
AZURE_STORAGE_ACCOUNT_NAME='...'

AZURE_FUNCTION_APP_NAME='...'


AZURE_ACCOUNT="$(az account show)"
AZURE_SUBSCRIPTION_ID="$(echo "$AZURE_ACCOUNT" | jq --raw-output '.id')"


# Create function app SP
#
# TODO: Remove this when Azure Function's Managed Service Identity when that becomes available for Linux Consumption apps.
az ad sp create-for-rbac --name "$AZURE_SP_NAME" --skip-assignment

AZURE_CLIENT_SECRET='...' # Save `password` - it will not appear again


# Create CNAME record
echo "Create CNAME record for $DOMAIN_NAME to $AZURE_CDN_ENDPOINT_NAME.azureedge.net"


# Create resource group
az group create --name "$AZURE_RESOURCE_GROUP_NAME" --location 'West US'


# Deploy resources
az group deployment create --resource-group "$AZURE_RESOURCE_GROUP_NAME" --template-file ./deployment-template.json --parameters "$(
    jq --null-input \
        --arg AZURE_APP_INSIGHTS_NAME "$AZURE_APP_INSIGHTS_NAME" \
        --arg AZURE_CDN_PROFILE_NAME "$AZURE_CDN_PROFILE_NAME" \
        --arg AZURE_CDN_ENDPOINT_NAME "$AZURE_CDN_ENDPOINT_NAME" \
        --arg AZURE_CDN_CUSTOM_DOMAIN_NAME "${DOMAIN_NAME//./-}" \
        --arg DOMAIN_NAME "$DOMAIN_NAME" \
        --arg AZURE_FUNCTION_APP_NAME "$AZURE_FUNCTION_APP_NAME" \
        --arg AZURE_FUNCTION_APP_SPID "$(az ad sp show --id "$AZURE_SP_NAME" --query objectId --output tsv)" \
        --arg AZURE_KEYVAULT_NAME "$AZURE_KEYVAULT_NAME" \
        --arg AZURE_STORAGE_ACCOUNT_NAME "$AZURE_STORAGE_ACCOUNT_NAME" \
        --arg SELF_OBJECT_ID "$(az ad signed-in-user show --query objectId --output tsv)" \
        '{
            "app_insights_name": { "value": $AZURE_APP_INSIGHTS_NAME },
            "cdn_profile_name": { "value": $AZURE_CDN_PROFILE_NAME },
            "cdn_endpoint_name": { "value": $AZURE_CDN_ENDPOINT_NAME },
            "cdn_custom_domain_name": { "value": $AZURE_CDN_CUSTOM_DOMAIN_NAME },
            "domain_name": { "value": $DOMAIN_NAME },
            "function_app_name": { "value": $AZURE_FUNCTION_APP_NAME },
            "function_app_spid": { "value": $AZURE_FUNCTION_APP_SPID },
            "keyvault_name": { "value": $AZURE_KEYVAULT_NAME },
            "storage_account_name": { "value": $AZURE_STORAGE_ACCOUNT_NAME },
            "self_object_id": { "value": $SELF_OBJECT_ID }
        }'
)"


# Get storage account connection string
AZURE_STORAGE_ACCOUNT_CONNECTION_STRING="$(
    az storage account show-connection-string \
        --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$AZURE_STORAGE_ACCOUNT_NAME" \
        --query connectionString --output tsv
)"


# Enable static website on storage account
az storage blob service-properties update \
    --connection-string "$AZURE_STORAGE_ACCOUNT_CONNECTION_STRING" --static-website true --index-document index.html --404-document 404.html

az storage container set-permission \
    --connection-string "$AZURE_STORAGE_ACCOUNT_CONNECTION_STRING" --name '$web' --public-access blob


# Grant permissions to function app SP

# Storage account
#
# TODO: Storage Blob Data Owner scoped to the $web container should be sufficient but isn't.
#       Azure complains that the operation requires write permission on the storage account itself.
#       Perhaps container-scoped permissions only apply when using container-scoped SAS rather than AAD?
az role assignment create \
    --assignee "$AZURE_SP_NAME" \
    --role 'Storage Account Contributor' \
    --scope "/subscriptions/$AZURE_SUBSCRIPTION_ID/resourceGroups/$AZURE_RESOURCE_GROUP_NAME/providers/Microsoft.Storage/storageAccounts/$AZURE_STORAGE_ACCOUNT_NAME"

# CDN
az role assignment create \
    --assignee "$AZURE_SP_NAME" \
    --role 'CDN Endpoint Contributor' \
    --scope "/subscriptions/$AZURE_SUBSCRIPTION_ID/resourceGroups/$AZURE_RESOURCE_GROUP_NAME/providers/Microsoft.Cdn/profiles/$AZURE_CDN_PROFILE_NAME/endpoints/$AZURE_CDN_ENDPOINT_NAME"


# Update function app configuration

az functionapp config appsettings set \
    --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$AZURE_FUNCTION_APP_NAME" \
    --settings \
        "APPINSIGHTS_INSTRUMENTATIONKEY=$(
            az resource show \
                --resource-group "$AZURE_RESOURCE_GROUP_NAME" --resource-type 'microsoft.insights/components' --name "$AZURE_APP_INSIGHTS_NAME" \
                --query 'properties.InstrumentationKey' --output tsv
        )" \
        "AzureWebJobsStorage=$AZURE_STORAGE_ACCOUNT_CONNECTION_STRING" \
        'FUNCTIONS_EXTENSION_VERSION=~2' \
        'FUNCTIONS_WORKER_RUNTIME=dotnet'
```


# Dependencies

Tools / packages:

- `azure-cli`
- `dotnet-sdk-2.2`
- [`azure-functions-core-tools`](https://github.com/Azure/azure-functions-core-tools/releases/latest) (`unzip` and add to `PATH`)


Third-party libraries:

- `Microsoft.NET.Sdk.Functions` - Functions SDK, required for the function parameter attribute types
- `Microsoft.Azure.WebJobs.Extensions.DurableTask` - Durable Functions SDK
- `Newtonsoft.Json` - JSON serialization and deserialization
- `Ply` - F# computation expression builder that creates `System.Threading.Tasks.Task` expressions, similar to F#'s native `Async` expressions. Needed for Durable Functions since those cannot use `Async`, and it's difficult to write non-trivial asynchronous code using `Task.ContinueWith` chains.

The code does *not* depend on the Azure .Net SDK or any ACME .Net implementation. The former in particular tens to pull in tens of megabytes of libraries and creates version conflicts with `Newtonsoft.Json`, which is not something I want to deal with. Instead, the code implements the minimum set of Azure and ACME protocol features that it needs, directly in terms of their respective web APIs.


# Local testing

1. Globals

    ```sh
    # See `Settings.fs` for an explanation of what each variable means.

    DOMAIN_NAME='...'

    ACME_DIRECTORY_URL='https://acme-v02.api.letsencrypt.org/directory'
    # Staging endpoint for local testing.
    # Note that the final step of uploading the certificate generated via this endpoint to Azure CDN will fail,
    # since Azure CDN does not recognize the staging endpoint's CA.
    # ACME_DIRECTORY_URL='https://acme-staging-v02.api.letsencrypt.org/directory'

    ACME_ACCOUNT_KEY_SECRET_NAME='...'
    ACME_CONTACT_URL='mailto:admin@example.com'

    AZURE_SP_NAME='http://...'

    AZURE_RESOURCE_GROUP_NAME='...'

    AZURE_CDN_PROFILE_NAME='...'
    AZURE_CDN_ENDPOINT_NAME='...'

    AZURE_KEYVAULT_NAME='...'
    AZURE_KEYVAULT_CERTIFICATE_NAME='...'

    # Must be unique for each function app. Multiple function apps cannot share the same storage account because of
    # https://github.com/Azure/azure-functions-host/issues/4499
    AZURE_STORAGE_ACCOUNT_NAME='...'

    AZURE_FUNCTION_APP_NAME='...'


    AZURE_ACCOUNT="$(az account show)"
    AZURE_SUBSCRIPTION_ID="$(echo "$AZURE_ACCOUNT" | jq --raw-output '.id')"
    ```


1. Generate `local.settings.json`

    ```sh
    >./local.settings.json jq --null-input --sort-keys \
        --arg AZURE_STORAGE_ACCOUNT_CONNECTION_STRING "$AZURE_STORAGE_ACCOUNT_CONNECTION_STRING" \
        --arg SECRET_SETTINGS "$(
            jq --null-input --sort-keys --compact-output \
                --arg DOMAIN_NAME "$DOMAIN_NAME" \
                --arg ACME_DIRECTORY_URL "$ACME_DIRECTORY_URL" \
                --arg ACME_ACCOUNT_KEY_SECRET_NAME "$ACME_ACCOUNT_KEY_SECRET_NAME" \
                --arg ACME_CONTACT_URL "$ACME_CONTACT_URL" \
                --arg AZURE_SUBSCRIPTION_ID "$AZURE_SUBSCRIPTION_ID" \
                --arg AZURE_RESOURCE_GROUP_NAME "$AZURE_RESOURCE_GROUP_NAME" \
                --arg AZURE_CLIENT_ID "$(az ad sp show --id "$AZURE_SP_NAME" --query appId --output tsv)" \
                --arg AZURE_CLIENT_SECRET "$AZURE_CLIENT_SECRET" \
                --arg AZURE_TENANT_ID "$(echo "$AZURE_ACCOUNT" | jq --raw-output '.tenantId')" \
                --arg AZURE_CDN_PROFILE_NAME "$AZURE_CDN_PROFILE_NAME" \
                --arg AZURE_CDN_ENDPOINT_NAME "$AZURE_CDN_ENDPOINT_NAME" \
                --arg AZURE_CDN_CUSTOM_DOMAIN_NAME "${DOMAIN_NAME//./-}" \
                --arg AZURE_KEYVAULT_NAME "$AZURE_KEYVAULT_NAME" \
                --arg AZURE_KEYVAULT_CERTIFICATE_NAME "$AZURE_KEYVAULT_CERTIFICATE_NAME" \
                --arg AZURE_STORAGE_ACCOUNT_NAME "$AZURE_STORAGE_ACCOUNT_NAME" \
                '{
                    "DomainName": $DOMAIN_NAME,
                    "AcmeDirectoryURL": $ACME_DIRECTORY_URL,
                    "AcmeAccountKeySecretName": $ACME_ACCOUNT_KEY_SECRET_NAME,
                    "AcmeContactURL": $ACME_CONTACT_URL,
                    "AzureSubscriptionId": $AZURE_SUBSCRIPTION_ID,
                    "AzureResourceGroupName": $AZURE_RESOURCE_GROUP_NAME,
                    "AzureClientID": $AZURE_CLIENT_ID,
                    "AzureClientSecret": $AZURE_CLIENT_SECRET,
                    "AzureTenantID": $AZURE_TENANT_ID,
                    "AzureCdnProfileName": $AZURE_CDN_PROFILE_NAME,
                    "AzureCdnEndpointName": $AZURE_CDN_ENDPOINT_NAME,
                    "AzureCdnCustomDomainName": $AZURE_CDN_CUSTOM_DOMAIN_NAME,
                    "AzureKeyVaultName": $AZURE_KEYVAULT_NAME,
                    "AzureKeyVaultCertificateName": $AZURE_KEYVAULT_CERTIFICATE_NAME,
                    "AzureStorageAccountName": $AZURE_STORAGE_ACCOUNT_NAME,
                }'
        )" \
        '{
            "IsEncrypted": false,
            "Values": {
                "AzureWebJobsStorage": $AZURE_STORAGE_ACCOUNT_CONNECTION_STRING,
                "FUNCTIONS_EXTENSION_VERSION": "~2",
                "FUNCTIONS_WORKER_RUNTIME": "dotnet",
                "SECRET_SETTINGS": $SECRET_SETTINGS
            }
        }'
    ```

1. Change timer triggers to HTTP triggers

    ```diff
    -([<Microsoft.Azure.WebJobs.TimerTrigger("0 0 0 * * *")>] timerInfo: Microsoft.Azure.WebJobs.TimerInfo)
    +([<Microsoft.Azure.WebJobs.HttpTrigger("Get")>] request: obj)
    ```

    ```diff
    -let _ = timerInfo
    +let _ = request
    ```


# Deploy

1. Set function app's `SECRET_SETTINGS` configuration to the same value as the string in `local.settings.json` (with appropriate modifications for production as necessary).

    ```sh
    az functionapp config appsettings set \
        --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$AZURE_FUNCTION_APP_NAME" \
        --settings \
            'SECRET_SETTINGS=...'
    ```

1. Change HTTP triggers to timer triggers

    ```diff
    -([<Microsoft.Azure.WebJobs.HttpTrigger("Get")>] request: obj)
    +([<Microsoft.Azure.WebJobs.TimerTrigger("0 0 0 * * *")>] timerInfo: Microsoft.Azure.WebJobs.TimerInfo)
    ```

    ```diff
    -let _ = request
    +let _ = timerInfo
    ```

1. Build

    (`func` does not support publishing F# projects, so we build it ourselves and publish the build output.)

    ```sh
    rm -rf bin/ obj/
    dotnet publish --configuration Release
    ```

1. Publish to Azure

    ```sh
    cd ./bin/Release/netcoreapp2.1/publish/ &&
        >./host.json jq --null-input --sort-keys \
            '{
                "version": "2.0",
                "logging": {
                    "applicationInsights": {
                        "samplingSettings": {
                            "isEnabled": false
                        }
                    }
                }
            }' &&
        >./local.settings.json echo '{ "IsEncrypted": false, "Values": { "FUNCTIONS_EXTENSION_VERSION": "~2", "FUNCTIONS_WORKER_RUNTIME": "dotnet" } }' &&
        func azure functionapp publish "$AZURE_FUNCTION_APP_NAME"
    ```


# Monitor (Application Insights)

- Traces

    ```
    traces
    | order by timestamp desc
    | limit 500
    | project timestamp, operation_Name, message
    ```

- Exceptions

    ```
    exceptions
    | order by timestamp desc
    | limit 50
    | project timestamp, operation_Name, innermostMessage, details
    ```


# Misc

- The function app must use a Linux function host, not Windows. This is because I could not figure out the way to make dotnet include the private key when it exports the final TLS certificate.

    This is only a problem with the Windows CAPI / CNG APIs, since non-exportable private keys is a Microsoft-specific feature ([`msPKI-Private-Key-Flag` attribute](https://docs.microsoft.com/en-us/openspecs/windows_protocols/ms-crtd/f6122d87-b999-4b92-bff8-f465e8949667)). openssl does not appear to bother with this, so dotnet on Linux does not have this problem.

- The ACME account key is generated with an ECDSA P-384 key. This is the most secure algorithm supported by Let's Encrypt.

- The TLS certificate is generated with an RSA 4096-bit key. Let's Encrypt and modern browsers also support ECDSA keys, but Azure CDN does not.
