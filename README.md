This repository contains an Azure Function that monitors a certificate in an Azure KeyVault and renews it before it expires using [the ACME v2 protocol.](https://tools.ietf.org/html/rfc8555) The Function is implemented in Rust and runs as [a custom handler.](https://learn.microsoft.com/en-us/azure/azure-functions/functions-custom-handlers)

This Function is used for the HTTPS certificate of <https://www.arnavion.dev>, which is served by an Azure CDN endpoint. [Let's Encrypt](https://letsencrypt.org/) is used as the ACME server.


# Build dependencies

- `azure-cli`
- `curl`
- `jq`
- `openssl`
- `podman`


# Setup

1. Define some global variables.

    ```sh
    # The top-level domain name. Certificates will be requested for "*.TOP_LEVEL_DOMAIN_NAME".
    export TOP_LEVEL_DOMAIN_NAME='arnavion.dev'

    # The resource group that will host the KeyVault and DNS zone.
    export AZURE_COMMON_RESOURCE_GROUP_NAME='arnavion-dev'

    # The ACME server's directory URL.
    export ACME_DIRECTORY_URL='https://acme-v02.api.letsencrypt.org/directory'

    # The contact URL used to register an account with the ACME server.
    export ACME_CONTACT_URL='mailto:admin@arnavion.dev'

    # The resource group that will host the Function app.
    export AZURE_ACME_RESOURCE_GROUP_NAME='arnavion-dev-acme'

    # The name of the KeyVault.
    export AZURE_KEY_VAULT_NAME='arnavion-dev-acme'

    # The name of the Function app.
    export AZURE_ACME_FUNCTION_APP_NAME='arnavion-dev-acme'

    # The name of the Storage Account used by the Function app.
    export AZURE_ACME_STORAGE_ACCOUNT_NAME='arnaviondevacme'

    # The name of the KeyVault key that will store the ACME account key.
    export AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME='letsencrypt-account-key'

    # The name of the Azure KeyVault certificate
    export AZURE_KEY_VAULT_CERTIFICATE_NAME='star-arnavion-dev'

    # The resource group that will host the Log Analytics workspace.
    export AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME='logs'

    # The Log Analytics workspace.
    export AZURE_LOG_ANALYTICS_WORKSPACE_NAME='arnavion-log-analytics'

    # The name of the Azure role used for the Function app.
    export AZURE_ACME_ROLE_NAME='functionapp-acme'


    export AZURE_ACCOUNT="$(az account show)"
    export AZURE_SUBSCRIPTION_ID="$(echo "$AZURE_ACCOUNT" | jq --raw-output '.id')"
    ```

1. Deploy Azure resources.

    ```sh
    # Create the resource group for the KeyVault and DNS zone.
    az group create --name "$AZURE_COMMON_RESOURCE_GROUP_NAME"


    # Create a KeyVault.
    az keyvault create \
        --resource-group "$AZURE_COMMON_RESOURCE_GROUP_NAME" --name "$AZURE_KEY_VAULT_NAME" \
        --enable-rbac-authorization


    # Create a DNS zone.
    az network dns zone create \
        --resource-group "$AZURE_COMMON_RESOURCE_GROUP_NAME" --name "$TOP_LEVEL_DOMAIN_NAME"


    # Create the resource group for the Function app.
    az group create --name "$AZURE_ACME_RESOURCE_GROUP_NAME"


    # Create a Storage account for the Function app.
    az storage account create \
        --resource-group "$AZURE_ACME_RESOURCE_GROUP_NAME" --name "$AZURE_ACME_STORAGE_ACCOUNT_NAME" \
        --sku 'Standard_LRS' --https-only --min-tls-version 'TLS1_2' --allow-blob-public-access false

    export AZURE_ACME_STORAGE_ACCOUNT_CONNECTION_STRING="$(
        az storage account show-connection-string \
            --resource-group "$AZURE_ACME_RESOURCE_GROUP_NAME" --name "$AZURE_ACME_STORAGE_ACCOUNT_NAME" \
            --query connectionString --output tsv
    )"


    # Create a Function app.
    az functionapp create \
        --resource-group "$AZURE_ACME_RESOURCE_GROUP_NAME" --name "$AZURE_ACME_FUNCTION_APP_NAME" \
        --storage-account "$AZURE_ACME_STORAGE_ACCOUNT_NAME" \
        --consumption-plan-location "$(az group show --name "$AZURE_ACME_RESOURCE_GROUP_NAME" --query location --output tsv)" \
        --functions-version '3' --os-type 'Linux' --runtime 'custom' \
        --assign-identity '[system]' \
        --disable-app-insights


    # Create a resource group for the Log Analytics workspace.
    az group create --name "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME"


    # Create a Log Analytics workspace.
    az monitor log-analytics workspace create \
        --resource-group "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME"


    # Configure the Function app to log to the Log Analytics workspace.
    az monitor diagnostic-settings create \
        --name 'logs' \
        --resource "$(
            az functionapp show \
                --resource-group "$AZURE_ACME_RESOURCE_GROUP_NAME" --name "$AZURE_ACME_FUNCTION_APP_NAME" \
                --query id --output tsv
        )" \
        --workspace "$(
            az monitor log-analytics workspace show \
                --resource-group "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
                --query id --output tsv
        )" \
        --logs '[{ "category": "FunctionAppLogs", "enabled": true }]'


    # Configure the KeyVault to log to the Log Analytics workspace.
    az monitor diagnostic-settings create \
        --name 'logs' \
        --resource "$(
            az keyvault show \
                --name "$AZURE_KEY_VAULT_NAME" \
                --query id --output tsv
        )" \
        --workspace "$(
            az monitor log-analytics workspace show \
                --resource-group "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
                --query id --output tsv
        )" \
        --logs '[{ "category": "AuditEvent", "enabled": true }]'


    # Create a custom role for the Function app to access the DNS zone, KeyVault and Log Analytics Workspace.
    az role definition create --role-definition "$(
        jq --null-input \
            --arg 'AZURE_ROLE_NAME' "$AZURE_ACME_ROLE_NAME" \
            --arg 'AZURE_SUBSCRIPTION_ID' "$AZURE_SUBSCRIPTION_ID" \
            '{
                "Name": $AZURE_ROLE_NAME,
                "AssignableScopes": [
                    "/subscriptions/\($AZURE_SUBSCRIPTION_ID)"
                ],
                "Actions": [
                    "Microsoft.Network/dnszones/read",
                    "Microsoft.Network/dnszones/TXT/delete",
                    "Microsoft.Network/dnszones/TXT/write",
                    "Microsoft.OperationalInsights/workspaces/read",
                    "Microsoft.OperationalInsights/workspaces/sharedKeys/action"
                ],
                "DataActions": [
                    "Microsoft.KeyVault/vaults/certificates/create/action",
                    "Microsoft.KeyVault/vaults/certificates/read",
                    "Microsoft.KeyVault/vaults/keys/create/action",
                    "Microsoft.KeyVault/vaults/keys/read",
                    "Microsoft.KeyVault/vaults/keys/sign/action"
                ],
            }'
    )"


    # Apply the role to the Function app
    function_app_identity="$(
        az functionapp identity show \
            --resource-group "$AZURE_ACME_RESOURCE_GROUP_NAME" --name "$AZURE_ACME_FUNCTION_APP_NAME" \
            --query principalId --output tsv
    )"
    for scope in \
        "$(az network dns zone show --resource-group "$AZURE_COMMON_RESOURCE_GROUP_NAME" --name "$TOP_LEVEL_DOMAIN_NAME" --query id --output tsv)" \
        "$(az network dns zone show --resource-group "$AZURE_COMMON_RESOURCE_GROUP_NAME" --name "$TOP_LEVEL_DOMAIN_NAME" --query id --output tsv)/TXT/_acme-challenge" \
        "$(az keyvault show --name "$AZURE_KEY_VAULT_NAME" --query id --output tsv)/keys/$AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME" \
        "$(az keyvault show --name "$AZURE_KEY_VAULT_NAME" --query id --output tsv)/certificates/$AZURE_KEY_VAULT_CERTIFICATE_NAME" \
        "$(
            az monitor log-analytics workspace show \
                --resource-group "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
                --query id --output tsv
        )"
    do
        az role assignment create \
            --role "$AZURE_ACME_ROLE_NAME" \
            --assignee "$function_app_identity" \
            --scope "$scope"
    done
    ```

1. If the Azure DNS zone is not your domain's primary nameserver, create NS records on your primary nameserver for the dns-01 challenge.

    ```sh
    echo "Create NS records for _acme-challenge.$TOP_LEVEL_DOMAIN_NAME. to:"; \
    az network dns zone show \
        --resource-group "$AZURE_COMMON_RESOURCE_GROUP_NAME" --name "$TOP_LEVEL_DOMAIN_NAME" \
        --query 'nameServers' --output tsv
    ```

1. Prepare the Log Analytics table schema.

    ```sh
    ./scripts/prepare-loganalytics-table-schema.sh
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
    export AZURE_ACME_SP_NAME="${TOP_LEVEL_DOMAIN_NAME//./-}-local-testing"

    az ad sp create-for-rbac --name "$AZURE_ACME_SP_NAME"

    export AZURE_ACME_CLIENT_SECRET='...' # Save `password` - it will not appear again
    export AZURE_ACME_CLIENT_ID="$(az ad sp list --display-name "$AZURE_ACME_SP_NAME" --query '[0].appId' --output tsv)"
    ```

1. Grant the SP access to the Azure resources.

    ```sh
    for scope in \
        "$(az network dns zone show --resource-group "$AZURE_COMMON_RESOURCE_GROUP_NAME" --name "$TOP_LEVEL_DOMAIN_NAME" --query id --output tsv)" \
        "$(az network dns zone show --resource-group "$AZURE_COMMON_RESOURCE_GROUP_NAME" --name "$TOP_LEVEL_DOMAIN_NAME" --query id --output tsv)/TXT/_acme-challenge" \
        "$(az keyvault show --name "$AZURE_KEY_VAULT_NAME" --query id --output tsv)/keys/$AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME" \
        "$(az keyvault show --name "$AZURE_KEY_VAULT_NAME" --query id --output tsv)/certificates/$AZURE_KEY_VAULT_CERTIFICATE_NAME" \
        "$(
            az monitor log-analytics workspace show \
                --resource-group "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
                --query id --output tsv
        )"
    do
        az role assignment create \
            --role "$AZURE_ACME_ROLE_NAME" \
            --assignee "$AZURE_ACME_CLIENT_ID" \
            --scope "$scope"
    done
    ```

1. Build the Function app and run it in the Functions host.

    ```sh
    ./build.sh debug

    curl -D - 'http://localhost:7071/renew-cert'
    ```

If you need to force a new certificate to be requested while the previous one in the KeyVault is still valid, delete it:

```sh
az keyvault certificate delete --vault-name "$AZURE_KEY_VAULT_NAME" --name "$AZURE_KEY_VAULT_CERTIFICATE_NAME"
sleep 10
az keyvault certificate purge --vault-name "$AZURE_KEY_VAULT_NAME" --name "$AZURE_KEY_VAULT_CERTIFICATE_NAME"
```

If you need to force the ACME server to return a new certificate even if a previous one is still valid, delete the account key:

```sh
az keyvault key delete --vault-name "$AZURE_KEY_VAULT_NAME" --name "$AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME"
sleep 10
az keyvault key purge --vault-name "$AZURE_KEY_VAULT_NAME" --name "$AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME"
```


# Deploy to Azure

```sh
./build.sh publish
```


# Monitor (Log Analytics)

## Function invocations

```
FunctionAppLogs_CL
| order by TimeGenerated desc, SequenceNumber_d desc
| where ObjectType_s == "function_invocation"
| project TimeGenerated, FunctionInvocationId_g, ObjectState_s
```

## Function invocation logs

```
FunctionAppLogs_CL
| order by TimeGenerated desc, SequenceNumber_d desc
| where FunctionInvocationId_g == "..."
| extend Record =
    case(
        isnotempty(Exception_s), Exception_s,
        isnotempty(Message), Message,
        strcat(
            ObjectType_s,
            iff(isempty(ObjectId_s), "", strcat("/", ObjectId_s)),
            iff(isempty(ObjectOperation_s), strcat(" is ", ObjectState_s), strcat(" does ", ObjectOperation_s, " ", ObjectValue_s))
        )
    )
| project TimeGenerated, Level, Record
```


# Misc

- The ACME account key is generated with an ECDSA P-384 key by default. This is the most secure algorithm supported by Let's Encrypt; Let's Encrypt does not support [P-521](https://github.com/letsencrypt/boulder/blob/9a4f0ca678e8c178e46200e7ef7599101851deeb/goodkey/good_key.go#L272) or [EdDSA keys.](https://github.com/letsencrypt/boulder/issues/4213)

  You can change the key algorithm in `build.sh` by changing the value of `"azure_key_vault_acme_account_key_type"` in the Function app secret settings.

- The TLS certificate is generated with an RSA 4096-bit key by default. You can change the key algorithm in `build.sh` by changing the value of `"azure_key_vault_certificate_key_type"` in the Function app secret settings.


# Old F# version

For the old F# version of this Function, see [the `fsharp` branch.](https://github.com/Arnavion/acme-azure-function/tree/fsharp) That version is no longer maintained.

The Rust version has a few differences compared to the F# version:

- The F# version had a bunch of dependency hell from Microsoft .Net libraries, like pulling multiple versions of `Newtonsoft.Json`

- The F# version used standard structural logging available to .Net Functions via the `Microsoft.Extensions.Logging` library. These logs were reported to App Insights / Log Analytics via the Functions host. The Rust version logs directly to Log Analytics instead, using its Data Collector API.

- The F# version worked with the ACME account key and cert private key in memory, and imported/exported them to/from KeyVault for this. The Rust version lets the KeyVault create the keys and uses KeyVault API to sign them.

- The F# version was limited to running on a Linux Consumption plan, due to .Net on Windows marking certificate private keys as non-exportable and preventing the cert from being used with Azure CDN. The Rust version does its crypto in pure Rust, so it does not have this limitation.

  However the build script in this repository still only builds a Linux binary. If you want to build a Windows binary, you'll need to adapt the script to your needs.


# License

AGPL-3.0-only

```
acme-azure-function

https://github.com/Arnavion/acme-azure-function

Copyright 2021 Arnav Singh

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as
published by the Free Software Foundation, version 3 of the
License.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
```
