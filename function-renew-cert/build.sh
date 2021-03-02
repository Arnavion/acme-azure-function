#!/bin/bash

set -euo pipefail

case "${1:-}" in
    debug-*)
        acme_directory_url="$ACME_STAGING_DIRECTORY_URL"
        ;;

    'publish')
        acme_directory_url="$ACME_DIRECTORY_URL"
        ;;
esac

./scripts/build.common.sh \
    "$1" \
    'renew-cert' \
    'function-renew-cert' \
    "$AZURE_ACME_RESOURCE_GROUP_NAME" \
    "$AZURE_ACME_CLIENT_ID" \
    "$AZURE_ACME_CLIENT_SECRET" \
    "$AZURE_ACME_STORAGE_ACCOUNT_CONNECTION_STRING" \
    "$AZURE_ACME_FUNCTION_APP_NAME" \
    '0 0 0 * * *' \
    "$(
        jq --null-input --sort-keys --compact-output \
            --arg AZURE_SUBSCRIPTION_ID "$AZURE_SUBSCRIPTION_ID" \
            --arg AZURE_LOG_ANALYTICS_WORKSPACE_ID "$AZURE_LOG_ANALYTICS_WORKSPACE_ID" \
            --arg AZURE_LOG_ANALYTICS_WORKSPACE_KEY "$AZURE_LOG_ANALYTICS_WORKSPACE_KEY" \
            --arg ACME_DIRECTORY_URL "$acme_directory_url" \
            --arg ACME_CONTACT_URL "$ACME_CONTACT_URL" \
            --arg AZURE_RESOURCE_GROUP_NAME "$AZURE_ACME_RESOURCE_GROUP_NAME" \
            --arg AZURE_KEY_VAULT_NAME "$AZURE_KEY_VAULT_NAME" \
            --arg AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME "$AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME" \
            --arg AZURE_KEY_VAULT_CERTIFICATE_NAME "$AZURE_KEY_VAULT_CERTIFICATE_NAME" \
            --arg TOP_LEVEL_DOMAIN_NAME "$TOP_LEVEL_DOMAIN_NAME" \
            '{
                "azure_subscription_id": $AZURE_SUBSCRIPTION_ID,
                "azure_log_analytics_workspace_id": $AZURE_LOG_ANALYTICS_WORKSPACE_ID,
                "azure_log_analytics_workspace_key": $AZURE_LOG_ANALYTICS_WORKSPACE_KEY,
                "acme_directory_url": $ACME_DIRECTORY_URL,
                "acme_contact_url": $ACME_CONTACT_URL,
                "azure_resource_group_name": $AZURE_RESOURCE_GROUP_NAME,
                "azure_key_vault_name": $AZURE_KEY_VAULT_NAME,
                "azure_key_vault_acme_account_key_name": $AZURE_KEY_VAULT_ACME_ACCOUNT_KEY_NAME,
                "azure_key_vault_acme_account_key_type": "ec:p384",
                "azure_key_vault_certificate_name": $AZURE_KEY_VAULT_CERTIFICATE_NAME,
                "azure_key_vault_certificate_key_type": "rsa:4096:exportable",
                "top_level_domain_name": $TOP_LEVEL_DOMAIN_NAME
            }'
    )"
