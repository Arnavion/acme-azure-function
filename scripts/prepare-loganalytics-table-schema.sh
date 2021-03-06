#!/bin/bash

# This script prepares the schema of the custom logs table in the Log Analytics workspace.
# It does this by pushing a row containing all the fields the logs might have, with dummy data to indicate the desired type.

set -euxo pipefail

azure_log_analytics_workspace_id="$(
    az monitor log-analytics workspace show \
        --resource-group "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
        --query 'customerId' --output tsv
)"

azure_log_analytics_workspace_key="$(
    az monitor log-analytics workspace get-shared-keys \
        --resource-group "$AZURE_LOG_ANALYTICS_WORKSPACE_RESOURCE_GROUP_NAME" --workspace-name "$AZURE_LOG_ANALYTICS_WORKSPACE_NAME" \
        --query 'primarySharedKey' --output tsv
)"

body="$(
    jq --null-input --compact-output \
        --arg TIME_COLLECTED "$(date -u '+%Y-%m-%dT%TZ')" \
        '[{
            "TimeCollected": $TIME_COLLECTED,
            "FunctionInvocationId": "30ebc19a-a995-4627-9518-93d251ee77c7",
            "SequenceNumber": 5,
            "Level": "Information",
            "Exception": "exception",
            "Message": "message",
            "ObjectType": "object_type",
            "ObjectId": "object_id",
            "ObjectOperation": "object_operation",
            "ObjectValue": "object_value",
            "ObjectState": "object_state",
        }]'
)"

x_ms_date="$(date -u '+%a, %d %b %Y %T GMT')"

authorization_header_value="SharedKey ${azure_log_analytics_workspace_id}:$(
    printf 'POST\n%d\napplication/json\nx-ms-date:%s\n/api/logs' "$(( "$(<<< "$body" wc -c)" - 1 ))" "$x_ms_date" |
        openssl sha256 -mac hmac -macopt "hexkey:$(<<< "$azure_log_analytics_workspace_key" base64 -d | xxd -ps -c 256)" -binary |
        base64 -w 0
)"

curl \
    -L \
    -D - --verbose \
    -H "authorization: $authorization_header_value" \
    -H 'content-type: application/json' \
    -H 'log-type: FunctionAppLogs' \
    -H 'time-generated-field: TimeCollected' \
    -H "x-ms-date: $x_ms_date" \
    --data-raw "$body" \
    "https://${azure_log_analytics_workspace_id}.ods.opinsights.azure.com/api/logs?api-version=2016-04-01"
