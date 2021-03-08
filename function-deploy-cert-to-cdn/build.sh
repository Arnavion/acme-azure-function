#!/bin/bash

set -euo pipefail

./scripts/build.common.sh \
    "$1" \
    'deploy-cert-to-cdn' \
    'function-deploy-cert-to-cdn' \
    "$AZURE_CDN_RESOURCE_GROUP_NAME" \
    "$AZURE_CDN_CLIENT_ID" \
    "$AZURE_CDN_CLIENT_SECRET" \
    "$AZURE_CDN_STORAGE_ACCOUNT_CONNECTION_STRING" \
    "$AZURE_CDN_FUNCTION_APP_NAME" \
    '0 0 1 * * *' \
    "$(
        jq --null-input --sort-keys --compact-output \
            --arg AZURE_SUBSCRIPTION_ID "$AZURE_SUBSCRIPTION_ID" \
            --arg AZURE_LOG_ANALYTICS_WORKSPACE_ID "$AZURE_LOG_ANALYTICS_WORKSPACE_ID" \
            --arg AZURE_LOG_ANALYTICS_WORKSPACE_KEY "$AZURE_LOG_ANALYTICS_WORKSPACE_KEY" \
            --arg AZURE_CDN_RESOURCE_GROUP_NAME "$AZURE_CDN_RESOURCE_GROUP_NAME" \
            --arg AZURE_CDN_PROFILE_NAME "$AZURE_CDN_PROFILE_NAME" \
            --arg AZURE_CDN_ENDPOINT_NAME "$AZURE_CDN_ENDPOINT_NAME" \
            --arg AZURE_CDN_CUSTOM_DOMAIN_NAME "${DOMAIN_NAME//./-}" \
            --arg AZURE_KEY_VAULT_RESOURCE_GROUP_NAME "$AZURE_ACME_RESOURCE_GROUP_NAME" \
            --arg AZURE_KEY_VAULT_NAME "$AZURE_KEY_VAULT_NAME" \
            --arg AZURE_KEY_VAULT_CERTIFICATE_NAME "$AZURE_KEY_VAULT_CERTIFICATE_NAME" \
            '{
                "azure_subscription_id": $AZURE_SUBSCRIPTION_ID,
                "azure_log_analytics_workspace_id": $AZURE_LOG_ANALYTICS_WORKSPACE_ID,
                "azure_log_analytics_workspace_key": $AZURE_LOG_ANALYTICS_WORKSPACE_KEY,
                "azure_cdn_resource_group_name": $AZURE_CDN_RESOURCE_GROUP_NAME,
                "azure_cdn_profile_name": $AZURE_CDN_PROFILE_NAME,
                "azure_cdn_endpoint_name": $AZURE_CDN_ENDPOINT_NAME,
                "azure_cdn_custom_domain_name": $AZURE_CDN_CUSTOM_DOMAIN_NAME,
                "azure_key_vault_resource_group_name": $AZURE_KEY_VAULT_RESOURCE_GROUP_NAME,
                "azure_key_vault_name": $AZURE_KEY_VAULT_NAME,
                "azure_key_vault_certificate_name": $AZURE_KEY_VAULT_CERTIFICATE_NAME,
            }'
    )"