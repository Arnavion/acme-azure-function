This function provisions a wildcard TLS certificate for a domain and stores it in an Azure KeyVault. The certificate is provisioned using [the ACME v2 protocol.](https://ietf-wg-acme.github.io/acme/draft-ietf-acme-acme.html) The account key used for the ACME v2 protocol is also stored in the same Azure KeyVault.


# Dependencies

Build-time tools:

- `azure-cli`
- `docker`


Third-party libraries:

- `Microsoft.NET.Sdk.Functions` - Functions SDK, required for the function parameter attribute types
- `Newtonsoft.Json` - JSON serialization and deserialization
- `Ply` - F# computation expression builder that creates `System.Threading.Tasks.Task` expressions, similar to F#'s native `Async` expressions.

The code does *not* depend on the Azure .Net SDK or any ACME .Net implementation. The former in particular tends to pull in tens of megabytes of libraries and creates version conflicts with `Newtonsoft.Json`, which is not something I want to deal with. Instead, the code implements the minimum set of Azure and ACME protocol features that it needs, directly in terms of their respective web APIs.


# Setup

1. Define some global variables.

    ```sh
    # The top-level domain name. Certificates will be requested for "*.TOP_LEVEL_DOMAIN_NAME".
    export TOP_LEVEL_DOMAIN_NAME='arnavion.dev'

    # The ACME server's directory URL.
    export ACME_DIRECTORY_URL='https://acme-v02.api.letsencrypt.org/directory'

    # The contact URL used to register an account with the ACME server.
    export ACME_CONTACT_URL='mailto:admin@arnavion.dev'

    # The name of the KeyVault secret that will store the ACME account key.
    export ACME_ACCOUNT_KEY_SECRET_NAME='arnavion-dev'

    # The resource group that will host the KeyVault and Function app.
    export AZURE_RESOURCE_GROUP_NAME='arnavion-dev'

    # The name of the KeyVault.
    export AZURE_KEYVAULT_NAME='arnavion-dev'

    # The name of the Function app.
    export AZURE_ACME_FUNCTION_APP_NAME='arnavion-dev'

    # The name of the Storage Account used by the Function app.
    export AZURE_ACME_STORAGE_ACCOUNT_NAME='arnavion-dev'

    # The name of the App Insights used to store the Function app's logs.
    export AZURE_APP_INSIGHTS_NAME='arnavion-dev'

    # The name of the Azure KeyVault certificate
    export AZURE_KEYVAULT_CERTIFICATE_NAME='arnavion-dev'


    export AZURE_ACCOUNT="$(az account show)"
    export AZURE_SUBSCRIPTION_ID="$(echo "$AZURE_ACCOUNT" | jq --raw-output '.id')"
    ```

1. Deploy Azure resources.

    - An Azure KeyVault to hold the ACME account key and the TLS certificate for the domain.

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
    az group deployment create --resource-group "$AZURE_RESOURCE_GROUP_NAME" --template-file ./AcmeFunction/deployment-template.json --parameters "$(
        jq --null-input \
            --arg TOP_LEVEL_DOMAIN_NAME "$TOP_LEVEL_DOMAIN_NAME" \
            --arg AZURE_KEYVAULT_NAME "$AZURE_KEYVAULT_NAME" \
            --arg AZURE_FUNCTION_APP_NAME "$AZURE_ACME_FUNCTION_APP_NAME" \
            --arg AZURE_STORAGE_ACCOUNT_NAME "$AZURE_ACME_STORAGE_ACCOUNT_NAME" \
            --arg AZURE_APP_INSIGHTS_NAME "$AZURE_APP_INSIGHTS_NAME" \
            '{
                "app_insights_name": { "value": $AZURE_APP_INSIGHTS_NAME },
                "function_app_name": { "value": $AZURE_FUNCTION_APP_NAME },
                "keyvault_name": { "value": $AZURE_KEYVAULT_NAME },
                "storage_account_name": { "value": $AZURE_STORAGE_ACCOUNT_NAME },
                "top_level_domain_name": { "value": $TOP_LEVEL_DOMAIN_NAME }
            }'
    )"


    # Get storage account connection string
    export AZURE_ACME_STORAGE_ACCOUNT_CONNECTION_STRING="$(
        az storage account show-connection-string \
            --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$AZURE_ACME_STORAGE_ACCOUNT_NAME" \
            --query connectionString --output tsv
    )"


    # Update function app configuration

    az functionapp config appsettings set \
        --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$AZURE_ACME_FUNCTION_APP_NAME" \
        --settings \
            "APPINSIGHTS_INSTRUMENTATIONKEY=$(
                az resource show \
                    --resource-group "$AZURE_RESOURCE_GROUP_NAME" --resource-type 'microsoft.insights/components' --name "$AZURE_APP_INSIGHTS_NAME" \
                    --query 'properties.InstrumentationKey' --output tsv
            )" \
            "AzureWebJobsStorage=$AZURE_ACME_STORAGE_ACCOUNT_CONNECTION_STRING" \
            'FUNCTIONS_EXTENSION_VERSION=~3' \
            'FUNCTIONS_WORKER_RUNTIME=dotnet'
    ```

1. Create an NS record with your DNS registrar for the dns-01 challenge.

    ```sh
    AZURE_DNS_SERVER="$(az network dns zone show --resource-group "$AZURE_RESOURCE_GROUP_NAME" --name "$TOP_LEVEL_DOMAIN_NAME" --query 'nameServers[0]' --output tsv)"
    echo "Create NS record for _acme-challenge.$TOP_LEVEL_DOMAIN_NAME. to $AZURE_DNS_SERVER"
    ```


# Test locally

1. Define some more global variables.

    ```sh
    # Staging endpoint for local testing.
    export ACME_STAGING_DIRECTORY_URL='https://acme-staging-v02.api.letsencrypt.org/directory'
    ```

1. Create a service principal.

    This is only needed for testing locally with the Azure Functions host `func`. When deployed to Azure, the Function app will use its Managed Service Identity instead.

    ```sh
    export AZURE_ACME_SP_NAME="http://${TOP_LEVEL_DOMAIN_NAME//./-}-local-testing"

    az ad sp create-for-rbac --name "$AZURE_ACME_SP_NAME" --skip-assignment

    export AZURE_ACME_CLIENT_SECRET='...' # Save `password` - it will not appear again
    export AZURE_ACME_CLIENT_ID="$(az ad sp show --id "$AZURE_ACME_SP_NAME" --query appId --output tsv)"
    ```

1. Grant the SP access to the Azure resources.

    ```sh
    # KeyVault
    az keyvault set-policy \
        --name "$AZURE_KEYVAULT_NAME" \
        --certificate-permissions get import --secret-permissions get set \
        --spn "$AZURE_ACME_SP_NAME"

    # DNS
    az role assignment create \
        --assignee "$AZURE_ACME_SP_NAME" \
        --role 'DNS Zone Contributor' \
        --scope "/subscriptions/$AZURE_SUBSCRIPTION_ID/resourceGroups/$AZURE_RESOURCE_GROUP_NAME/providers/Microsoft.Network/dnsZones/$TOP_LEVEL_DOMAIN_NAME"
    ```

1. Build the function app and run it in the functions host.

    ```sh
    ./AcmeFunction/build.sh debug

    curl -D - 'http://localhost:7071/api/OrchestratorManager'
    ```

If you need to force a new certificate to be requested while the previous one in the KeyVault is still valid, delete it:

```sh
az keyvault certificate delete --vault-name "$AZURE_KEYVAULT_NAME" --name "$AZURE_KEYVAULT_CERTIFICATE_NAME"
```

If you need to force the ACME server to return a new certificate even if a previous one is still valid, delete the account key:

```sh
az keyvault secret delete --vault-name "$AZURE_KEYVAULT_NAME" --name "$ACME_ACCOUNT_KEY_SECRET_NAME"
```


# Deploy to Azure

1. Build.

    ```sh
    rm -rf ./AcmeFunction/{obj,bin}/
    ./AcmeFunction/build.sh release
    ```

1. Publish to Azure.

    ```sh
    ./AcmeFunction/build.sh publish
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
