This function monitors an Azure CDN endpoint's custom domain configured to use an HTTPS certificate from an Azure KeyVault. It ensures the CDN endpoint is using the latest certificate in the KeyVault.


# Dependencies

Build-time tools:

- `azure-cli`
- `docker`


Third-party libraries:

- `Microsoft.NET.Sdk.Functions` - Functions SDK, required for the function parameter attribute types
- `Newtonsoft.Json` - JSON serialization and deserialization
- `Ply` - F# computation expression builder that creates `System.Threading.Tasks.Task` expressions, similar to F#'s native `Async` expressions.

The code does *not* depend on the Azure .Net SDK. It tends to pull in tens of megabytes of libraries and creates version conflicts with `Newtonsoft.Json`, which is not something I want to deal with. Instead, the code implements the minimum set of Azure protocol features that it needs, directly in terms of their respective web APIs.


# Setup

1. Define some global variables.

    ```sh
    # The custom domain served by the CDN endpoint.
    export DOMAIN_NAME='www.arnavion.dev'

    # The resource group that will host the KeyVault and Function app.
    export AZURE_RESOURCE_GROUP_NAME='arnavion-dev'

    # The name of the CDN profile.
    export AZURE_CDN_PROFILE_NAME='arnavion-dev'

    # The name of the CDN endpoint.
    export AZURE_CDN_ENDPOINT_NAME='www-arnavion-dev'

    # The name of the KeyVault that the certificate will be looked up from.
    export AZURE_KEYVAULT_NAME='arnavion-dev'

    # The name of the Azure KeyVault certificate
    export AZURE_KEYVAULT_CERTIFICATE_NAME='arnavion-dev'

    # The name of the Function app.
    export AZURE_CDN_FUNCTION_APP_NAME='www-arnavion-dev'

    # The name of the Storage Account used by the Function app.
    export AZURE_CDN_STORAGE_ACCOUNT_NAME='www-arnavion-dev'

    # The name of the App Insights used to store the Function app's logs.
    export AZURE_APP_INSIGHTS_NAME='arnavion-dev'


    export AZURE_ACCOUNT="$(az account show)"
    export AZURE_SUBSCRIPTION_ID="$(echo "$AZURE_ACCOUNT" | jq --raw-output '.id')"
    ```

1. Deploy Azure resources.

    - An Azure CDN profile and endpoint.

    - A custom domain on the CDN endpoint.

    - An Azure KeyVault to hold the ACME account key and the TLS certificate for the custom domain.

    - An Azure Function app.

    - An Azure storage account used as storage for the Azure Function app.

    ```sh
    # Create resource group
    az group create --name "$AZURE_RESOURCE_GROUP_NAME" --location 'West US'


    # Create the KeyVault.
    #
    # (This is created manually here instead of in the deployment template below because it is not possible to deploy a KeyVault without overwriting
    # the existing KeyVault's access policies.)
    az keyvault create --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$AZURE_KEYVAULT_NAME"


    # Deploy resources
    az group deployment create --resource-group "$AZURE_RESOURCE_GROUP_NAME" --template-file ./UpdateCdnCertificateFunction/deployment-template.json --parameters "$(
        jq --null-input \
            --arg DOMAIN_NAME "$DOMAIN_NAME" \
            --arg AZURE_CDN_PROFILE_NAME "$AZURE_CDN_PROFILE_NAME" \
            --arg AZURE_CDN_ENDPOINT_NAME "$AZURE_CDN_ENDPOINT_NAME" \
            --arg AZURE_CDN_CUSTOM_DOMAIN_NAME "${DOMAIN_NAME//./-}" \
            --arg AZURE_KEYVAULT_NAME "$AZURE_KEYVAULT_NAME" \
            --arg AZURE_FUNCTION_APP_NAME "$AZURE_CDN_FUNCTION_APP_NAME" \
            --arg AZURE_STORAGE_ACCOUNT_NAME "$AZURE_CDN_STORAGE_ACCOUNT_NAME" \
            --arg AZURE_APP_INSIGHTS_NAME "$AZURE_APP_INSIGHTS_NAME" \
            '{
                "app_insights_name": { "value": $AZURE_APP_INSIGHTS_NAME },
                "cdn_custom_domain_name": { "value": $AZURE_CDN_CUSTOM_DOMAIN_NAME },
                "cdn_endpoint_name": { "value": $AZURE_CDN_ENDPOINT_NAME },
                "cdn_profile_name": { "value": $AZURE_CDN_PROFILE_NAME },
                "domain_name": { "value": $DOMAIN_NAME },
                "function_app_name": { "value": $AZURE_FUNCTION_APP_NAME },
                "keyvault_name": { "value": $AZURE_KEYVAULT_NAME },
                "storage_account_name": { "value": $AZURE_STORAGE_ACCOUNT_NAME }
            }'
    )"


    # Get storage account connection string
    export AZURE_CDN_STORAGE_ACCOUNT_CONNECTION_STRING="$(
        az storage account show-connection-string \
            --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_STORAGE_ACCOUNT_NAME" \
            --query connectionString --output tsv
    )"


    # Update function app configuration

    az functionapp config appsettings set \
        --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$AZURE_CDN_FUNCTION_APP_NAME" \
        --settings \
            "APPINSIGHTS_INSTRUMENTATIONKEY=$(
                az resource show \
                    --resource-group "$AZURE_RESOURCE_GROUP_NAME" --resource-type 'microsoft.insights/components' --name "$AZURE_APP_INSIGHTS_NAME" \
                    --query 'properties.InstrumentationKey' --output tsv
            )" \
            "AzureWebJobsStorage=$AZURE_CDN_STORAGE_ACCOUNT_CONNECTION_STRING" \
            'FUNCTIONS_EXTENSION_VERSION=~3' \
            'FUNCTIONS_WORKER_RUNTIME=dotnet'
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
    # KeyVault
    az keyvault set-policy \
        --name "$AZURE_KEYVAULT_NAME" \
        --certificate-permissions get \
        --spn "$AZURE_CDN_SP_NAME"

    # CDN
    az role assignment create \
        --assignee "$AZURE_CDN_SP_NAME" \
        --role 'CDN Endpoint Contributor' \
        --scope "/subscriptions/$AZURE_SUBSCRIPTION_ID/resourceGroups/$AZURE_RESOURCE_GROUP_NAME/providers/Microsoft.Cdn/profiles/$AZURE_CDN_PROFILE_NAME/endpoints/$AZURE_CDN_ENDPOINT_NAME"
    ```

1. Build the function app and run it in the functions host.

    ```sh
    ./UpdateCdnCertificateFunction/build.sh debug

    curl -D - 'http://localhost:7071/api/UpdateCdnCertificate'
    ```


# Deploy to Azure

1. Build.

    ```sh
    rm -rf ./UpdateCdnCertificateFunction/{obj,bin}/
    ./UpdateCdnCertificateFunction/build.sh release
    ```

1. Publish to Azure.

    ```sh
    ./UpdateCdnCertificateFunction/build.sh publish
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
