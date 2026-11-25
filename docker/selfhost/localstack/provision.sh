#!/bin/bash
# Provision the AWS resources the services expect in LocalStack: SQS queues,
# S3 buckets, DynamoDB tables and KMS keys. LocalStack runs this once it is
# ready (/etc/localstack/init/ready.d) and again after a restart that lost its
# state, so every step is idempotent.
#
# Mirrors tooling/xtask/crates/xtask_local/src/local/{resources,localstack}.rs,
# which provision the same resources for `just run_local`. Keep the two in sync
# when a queue, bucket or table is added.
set -euo pipefail

ACCOUNT_ID=000000000000
REGION=us-east-1
export AWS_DEFAULT_REGION="$REGION"

log() { echo "[macro-provision] $*"; }

# Each awslocal call starts a Python CLI, about two seconds apiece, so the
# independent calls below run in parallel: `par` starts one in the background
# and `join` waits for all of them, failing if any did.
PIDS=()
par() { "$@" & PIDS+=("$!"); }
join() {
  local pid failed=0
  for pid in "${PIDS[@]}"; do wait "$pid" || failed=1; done
  PIDS=()
  return "$failed"
}

queue_arn() { echo "arn:aws:sqs:${REGION}:${ACCOUNT_ID}:$1"; }

# --- SQS ---------------------------------------------------------------------

QUEUES=(
  slack-import-queue
  slack-import-dlq
  notification-queue
  notification-ingress-queue
  push-delivery-queue
  webhook-event-queue.fifo
  email-service-backfill-queue
  email-service-crm-cleanup-queue
  email-service-crm-population-queue.fifo
  delete-chat-handler-queue
  contacts-queue
  convert-service-queue
  delete-document-handler-queue
  document-upload-finalizer-queue
  document-text-extractor-lambda-queue
  email-service-scheduled-queue
  email-service-gmail-inbox-sync-queue
  email-service-gmail-inbox-retry-queue
  email-service-gmail-ops-queue
  email-service-gmail-ops-retry-queue
  email-service-refresh-queue
  email-service-link-maintenance-queue
  search-event-queue
  email-sfs-delete-queue
  email-service-sfs-mapper-queue
  static-file-s3-event-notification-queue
  reminder-dispatch-queue
  calendar-reminder-dispatch-queue
  calendar-service-backfill-queue
)

create_queue() {
  if [[ "$1" == *.fifo ]]; then
    awslocal sqs create-queue --queue-name "$1" --attributes FifoQueue=true >/dev/null
  else
    awslocal sqs create-queue --queue-name "$1" >/dev/null
  fi
}

for queue in "${QUEUES[@]}"; do par create_queue "$queue"; done
join

# Non-default attributes. Applied after every queue exists so redrive targets
# resolve, and to existing queues too.
set_attrs() {
  local queue="$1"; shift
  awslocal sqs set-queue-attributes \
    --queue-url "http://localhost:4566/${ACCOUNT_ID}/${queue}" \
    --attributes "$1" >/dev/null
}

par set_attrs slack-import-queue "{\"VisibilityTimeout\":\"900\",\"RedrivePolicy\":\"{\\\"deadLetterTargetArn\\\":\\\"$(queue_arn slack-import-dlq)\\\",\\\"maxReceiveCount\\\":5}\"}"
par set_attrs slack-import-dlq '{"MessageRetentionPeriod":"1209600"}'
par set_attrs email-service-crm-population-queue.fifo '{"VisibilityTimeout":"60","DelaySeconds":"10"}'
par set_attrs email-service-refresh-queue '{"VisibilityTimeout":"300"}'
par set_attrs email-service-link-maintenance-queue '{"VisibilityTimeout":"300"}'
join
log "queues ready"

# --- S3 ----------------------------------------------------------------------

BUCKETS=(
  macro-email-attachments
  doc-storage
  docx-upload
  static-file-storage
  bulk-upload-staging
  macro-call-recording-local
  agent-session-changes
  github-pull-request-patches
)

CORS='{"CORSRules":[{"AllowedOrigins":["*"],"AllowedMethods":["GET","PUT","POST","DELETE","HEAD"],"AllowedHeaders":["*"],"ExposeHeaders":["ETag"],"MaxAgeSeconds":3600}]}'

create_bucket() {
  awslocal s3api create-bucket --bucket "$1" >/dev/null
  awslocal s3api put-bucket-cors --bucket "$1" --cors-configuration "$CORS"
}

for bucket in "${BUCKETS[@]}"; do par create_bucket "$bucket"; done
join

# doc-storage ObjectCreated events publish to the upload finalizer queue.
FINALIZER_ARN="$(queue_arn document-upload-finalizer-queue)"
awslocal sqs set-queue-attributes \
  --queue-url "http://localhost:4566/${ACCOUNT_ID}/document-upload-finalizer-queue" \
  --attributes "{\"Policy\":\"{\\\"Version\\\":\\\"2012-10-17\\\",\\\"Statement\\\":[{\\\"Effect\\\":\\\"Allow\\\",\\\"Principal\\\":\\\"*\\\",\\\"Action\\\":\\\"sqs:SendMessage\\\",\\\"Resource\\\":\\\"${FINALIZER_ARN}\\\",\\\"Condition\\\":{\\\"ArnEquals\\\":{\\\"aws:SourceArn\\\":\\\"arn:aws:s3:::doc-storage\\\"}}}]}\"}" >/dev/null
awslocal s3api put-bucket-notification-configuration --bucket doc-storage \
  --notification-configuration "{\"QueueConfigurations\":[{\"Id\":\"document-upload-finalizer\",\"QueueArn\":\"${FINALIZER_ARN}\",\"Events\":[\"s3:ObjectCreated:*\"]}]}"
log "buckets ready"

# --- DynamoDB ----------------------------------------------------------------

create_table() {
  local name="$1"; shift
  if awslocal dynamodb describe-table --table-name "$name" >/dev/null 2>&1; then
    return
  fi
  awslocal dynamodb create-table --table-name "$name" --billing-mode PAY_PER_REQUEST "$@" >/dev/null
}

par create_table bulk-upload \
  --attribute-definitions AttributeName=PK,AttributeType=S AttributeName=SK,AttributeType=S \
  --key-schema AttributeName=PK,KeyType=HASH AttributeName=SK,KeyType=RANGE \
  --global-secondary-indexes 'IndexName=DocumentPkIndex,KeySchema=[{AttributeName=SK,KeyType=HASH}],Projection={ProjectionType=ALL}'

par create_table connection-gateway-table \
  --attribute-definitions AttributeName=PK,AttributeType=S AttributeName=SK,AttributeType=S \
  --key-schema AttributeName=PK,KeyType=HASH AttributeName=SK,KeyType=RANGE \
  --global-secondary-indexes 'IndexName=ConnectionPkIndex,KeySchema=[{AttributeName=SK,KeyType=HASH},{AttributeName=PK,KeyType=RANGE}],Projection={ProjectionType=ALL}'

par create_table static-file-metadata \
  --attribute-definitions AttributeName=file_id,AttributeType=S \
  --key-schema AttributeName=file_id,KeyType=HASH
join
log "tables ready"

# --- KMS ---------------------------------------------------------------------

# Addressed by alias: CreateKey mints a random key id on every run, and KMS
# accepts an alias anywhere a key id goes.
create_key() {
  local alias="$1" description="$2"
  if awslocal kms list-aliases --query "Aliases[?AliasName=='${alias}'] | length(@)" --output text | grep -qx 0; then
    local key_id
    key_id="$(awslocal kms create-key --description "$description" --query KeyMetadata.KeyId --output text)"
    awslocal kms create-alias --alias-name "$alias" --target-key-id "$key_id"
  fi
}

par create_key alias/macro-local-cursor-api-key   "Cursor API key encryption key"
par create_key alias/macro-local-codex-oauth      "ChatGPT OAuth envelope encryption key"
par create_key alias/macro-local-mcp-oauth        "MCP OAuth authorization code encryption key"
par create_key alias/macro-local-microsoft-token  "Microsoft refresh token envelope encryption key"
par create_key alias/whatsapp-session-service-local "WhatsApp session data key encryption key"
join
log "kms keys ready"
