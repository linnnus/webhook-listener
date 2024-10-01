#!/bin/sh

# Expects service to be listening on socket
# Expects to be run from project root
# Payload signed with 'mysecret'

curl --unix-socket /tmp/websocket-listener.sock http://localhost/ \
	-X POST \
	--data @./examples/sample_push_payload.json \
	-H 'X-Github-Event: push' \
	-H 'X-Hub-Signature-256: sha256=6803d2a3e495fc4bd286d428ea4b794476a1ff1b72bbea4dfafd2477d5d89188' \
	-H 'Content-Length: 7413' \
	-H 'Content-Type: application/json' \
	-v
