This Function monitors an Azure CDN endpoint's custom domain configured to use an HTTPS certificate from an Azure KeyVault. It ensures the CDN endpoint is using the latest certificate in the KeyVault.


# Dependencies

Build-time tools:

- `azure-cli`
- `docker`

This Function is implemented in Rust and runs as [a custom handler.](https://docs.microsoft.com/en-us/azure/azure-functions/functions-custom-handlers)


# Setup

1. Define some global variables.

    ```sh
    # The custom domain served by the CDN endpoint.
    export DOMAIN_NAME='www.arnavion.dev'

    # The resource group that will host the KeyVault and Function app.
    export AZURE_CDN_RESOURCE_GROUP_NAME='arnavion-dev-www'

    # The name of the CDN profile.
    export AZURE_CDN_PROFILE_NAME='cdn-profile'

    # The name of the CDN endpoint.
    export AZURE_CDN_ENDPOINT_NAME='www-arnavion-dev'

    # The name of the KeyVault that the certificate will be looked up from.
    export AZURE_KEY_VAULT_NAME='arnavion-dev-acme'

    # The name of the Azure KeyVault certificate
    export AZURE_KEY_VAULT_CERTIFICATE_NAME='arnavion-dev'

    # The name of the Function app.
    export AZURE_CDN_FUNCTION_APP_NAME='arnavion-dev-www'

    # The name of the Storage Account used by the Function app.
    export AZURE_CDN_STORAGE_ACCOUNT_NAME='wwwarnaviondev'

    # The resource group that will host the Log Analytics workspace.
    export AZURE_MONITOR_RESOURCE_GROUP_NAME='logs'

    # The Log Analytics workspace.
    export AZURE_LOG_ANALYTICS_WORKSPACE_NAME='arnavion-log-analytics'


    export AZURE_ACCOUNT="$(az account show)"
    export AZURE_SUBSCRIPTION_ID="$(echo "$AZURE_ACCOUNT" | jq --raw-output '.id')"
    ```

1. Create a CNAME record with your DNS registrar for the CDN endpoint.

    ```sh
    echo "Create CNAME record for $DOMAIN_NAME. to $AZURE_CDN_ENDPOINT_NAME.azureedge.net."
    ```

1. Deploy Azure resources.

    - An Azure CDN profile and endpoint.

    - A custom domain on the CDN endpoint.

    - An Azure KeyVault to hold the ACME account key and the TLS certificate for the custom domain.

    - An Azure Function app.

    - An Azure storage account used as storage for the Azure Function app.

    ```sh
    # Create a resource group.
    az group create --name "$AZURE_CDN_RESOURCE_GROUP_NAME"


    # Create a Storage account for the website and the Function app.
    az storage account create \
        --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_STORAGE_ACCOUNT_NAME" \
        --sku 'Standard_LRS' --https-only --min-tls-version 'TLS1_2' --allow-blob-public-access false

    storage_account_web_endpoint="$(
        az storage account show \
            --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_STORAGE_ACCOUNT_NAME" \
            --query 'primaryEndpoints.web' --output tsv |
            sed -Ee 's|^https://(.*)/$|\1|'
    )"

    export AZURE_CDN_STORAGE_ACCOUNT_CONNECTION_STRING="$(
        az storage account show-connection-string \
            --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_STORAGE_ACCOUNT_NAME" \
            --query connectionString --output tsv
    )"


    # Register the CDN service principal "Microsoft.AzureFrontDoor-Cdn"
    az ad sp create --id '205478c0-bd83-4e1b-a9d6-db63a3e1e1c8'


    # Create a CDN.
    az cdn profile create \
        --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_PROFILE_NAME" \
        --sku 'Standard_Microsoft'

    az cdn endpoint create \
        --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --profile-name "$AZURE_CDN_PROFILE_NAME" --name "$AZURE_CDN_ENDPOINT_NAME" \
        --origin "$storage_account_web_endpoint" \
        --origin-host-header "$storage_account_web_endpoint" \
        --enable-compression \
        --no-http \
        --query-string-caching-behavior 'IgnoreQueryString'

    az cdn custom-domain create \
        --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" \
        --profile-name "$AZURE_CDN_PROFILE_NAME" --endpoint-name "$AZURE_CDN_ENDPOINT_NAME" --name "${DOMAIN_NAME//./-}" \
        --hostname "$DOMAIN_NAME"


    # Create a Function app.
    az functionapp create \
        --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_FUNCTION_APP_NAME" \
        --storage-account "$AZURE_CDN_STORAGE_ACCOUNT_NAME" \
        --consumption-plan-location "$(az group show --name "$AZURE_CDN_RESOURCE_GROUP_NAME" --query location --output tsv)" \
        --functions-version '3' --os-type 'Linux' --runtime 'custom' \
        --assign-identity '[system]' \
        --disable-app-insights

    function_app_identity="$(
        az functionapp identity show --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_FUNCTION_APP_NAME" --query principalId --output tsv
    )"


    # Give the Function app access to the CDN.
    az role assignment create \
        --role 'CDN Endpoint Contributor' \
        --assignee "$function_app_identity" \
        --scope "$(
            az cdn endpoint show \
                --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --profile-name "$AZURE_CDN_PROFILE_NAME" --name "$AZURE_CDN_ENDPOINT_NAME" \
                --query id --output tsv
        )"


    # Give the Function app access to the KeyVault.
    az keyvault set-policy --name "$AZURE_KEY_VAULT_NAME" \
        --object-id "$function_app_identity" \
        --certificate-permissions 'get'


    # Give the CDN access to the KeyVault.
    az keyvault set-policy --name "$AZURE_KEY_VAULT_NAME" \
        --object-id "$(
            az ad sp list --display-name 'Microsoft.AzureFrontDoor-Cdn' --query '[0].objectId' --output tsv
        )" \
        --secret-permissions 'get'


    # Create a resource group for the Log Analytics workspace.
    az group create --name "$AZURE_MONITOR_RESOURCE_GROUP_NAME"


    # Create a Log Analytics workspace.
    az monitor log-analytics workspace create \
        --resource-group "$AZURE_MONITOR_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME"


    # Configure the CDN to log to the Log Analytics workspace.
    az monitor diagnostic-settings create \
        --name 'logs' \
        --resource "$(
            az cdn profile show \
                --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_PROFILE_NAME" \
                --query id --output tsv
        )" \
        --workspace "$(
            az monitor log-analytics workspace show \
                --resource-group "$AZURE_MONITOR_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
                --query id --output tsv
        )" \
        --logs '[{ "category": "AzureCdnAccessLog", "enabled": true }]'


    # Configure the Function app to log to the Log Analytics workspace.
    az monitor diagnostic-settings create \
        --name 'logs' \
        --resource "$(
            az functionapp show \
                --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_FUNCTION_APP_NAME" \
                --query id --output tsv
        )" \
        --workspace "$(
            az monitor log-analytics workspace show \
                --resource-group "$AZURE_MONITOR_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
                --query id --output tsv
        )" \
        --logs '[{ "category": "FunctionAppLogs", "enabled": true }]'
    ```


# Test locally

1. Create a service principal.

    This is only needed for testing locally with the Azure Functions host `func`. When deployed to Azure, the Function app will use its Managed Service Identity instead.

    ```sh
    export AZURE_CDN_SP_NAME="http://${AZURE_CDN_ENDPOINT_NAME}-local-testing"

    az ad sp create-for-rbac --name "$AZURE_CDN_SP_NAME" --skip-assignment

    export AZURE_CDN_CLIENT_SECRET='...' # Save `password` - it will not appear again
    export AZURE_CDN_CLIENT_ID="$(az ad sp show --id "$AZURE_CDN_SP_NAME" --query appId --output tsv)"
    ```

1. Grant the SP access to the Azure resources.

    ```sh
    # CDN
    az role assignment create \
        --role 'CDN Endpoint Contributor' \
        --assignee "$AZURE_CDN_SP_NAME" \
        --scope "$(
            az cdn endpoint show \
                --resource-group "$AZURE_CDN_RESOURCE_GROUP_NAME" --profile-name "$AZURE_CDN_PROFILE_NAME" --name "$AZURE_CDN_ENDPOINT_NAME" \
                --query id --output tsv
        )"


    # KeyVault
    az keyvault set-policy \
        --name "$AZURE_KEY_VAULT_NAME" \
        --certificate-permissions get \
        --spn "$AZURE_CDN_SP_NAME"
    ```

1. Build the Function app and run it in the Functions host.

    ```sh
    ./update-cdn-cert/build.sh debug

    curl -D - 'http://localhost:7071/update-cdn-cert'
    ```


# Deploy to Azure

```sh
./update-cdn-cert/build.sh publish
```


# Monitor (Log Analytics)

```
FunctionAppLogs
| where TimeGenerated > now(-7d)
| order by TimeGenerated desc
| project TimeGenerated, Category, Level, Message
```


# Misc

- Azure CDN does not support certificates with ECDSA keys, only RSA keys. It also requires the private key to be exportable, which precludes the key from being stored in HSMs.

  If you're generating the cert using the `acme` function in this repo, ensure that you set `"azure_key_vault_certificate_key_type"` in the Function app secret settings to `"rsa-2048:exportable"` or `"rsa-4096:exportable"`, not to any of `"rsa:2048"`, `"rsa:4096"`, `"rsa-hsm:*"`, `"ec:*"` or `"ec-hsm:*"`.
