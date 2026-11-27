#!/usr/bin/env bash

set -euo pipefail

repository_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
bilin_source="${1:-${BILIN_SOURCE:-"${repository_root}/.."}}"
proto_root="${bilin_source}/bilin-proto/proto"

if [[ ! -f "${proto_root}/api_instance.proto" ]]; then
    echo "Bilin proto source not found at ${proto_root}" >&2
    exit 1
fi

if [[ "$(protoc --version)" != "libprotoc 35.1" && "$(protoc --version)" != "libprotoc 36.1" && "$(protoc --version)" != "libprotoc 36.2" ]]; then
    echo "protoc 35.1, 36.1 or 36.2 is required" >&2
    exit 1
fi
if [[ "$(protoc-gen-go --version)" != "protoc-gen-go v1.36.11" ]]; then
    echo "protoc-gen-go v1.36.11 is required" >&2
    exit 1
fi

protoc \
    -I "${proto_root}" \
    --go_out="${repository_root}" \
    --go_opt=module=github.com/AmashiroNatsuki250/Bilin/bilin-go \
    --go_opt=Mannotations.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/bilin \
    --go_opt=Mcore_config.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/core_config \
    --go_opt=Mcommon.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/common \
    --go_opt=Merror.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/error \
    --go_opt=Macl.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/acl \
    --go_opt=Mpeer_rpc.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/peer_rpc \
    --go_opt=Mapi_instance.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/api/instance \
    --go_opt=Mapi_config.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/api/config \
    --go_opt=Mapi_manage.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/api/manage \
    --go_opt=Mweb.proto=github.com/AmashiroNatsuki250/Bilin/bilin-go/proto/web \
    "${proto_root}/annotations.proto" \
    "${proto_root}/common.proto" \
    "${proto_root}/core_config.proto" \
    "${proto_root}/error.proto" \
    "${proto_root}/acl.proto" \
    "${proto_root}/peer_rpc.proto" \
    "${proto_root}/api_instance.proto" \
    "${proto_root}/api_config.proto" \
    "${proto_root}/api_manage.proto" \
    "${proto_root}/web.proto"
