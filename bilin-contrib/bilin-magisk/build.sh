#!/bin/sh
WEBUI_SOURCE="../../bilin-web/config-generator/dist"

if [ -f "${WEBUI_SOURCE}/index.html" ]; then
    rm -rf ./webroot
    mkdir -p ./webroot
    cp -R "${WEBUI_SOURCE}/." ./webroot/
elif [ ! -f "./webroot/index.html" ]; then
    echo "Error: WebUI 构建产物不存在，请先运行 pnpm --dir ../../bilin-web/config-generator build."
    exit 1
fi

version=$(grep '^version =' ../../bilin/Cargo.toml | cut -d '"' -f 2)

if [ -z "$version" ]; then
    echo "Error: 版本号不存在."
    exit 1
fi
version="v${version}"

filename="bilin_magisk_${version}.zip"
echo "${version}"

if [ ! -f "./bilin-core" ] || [ ! -f "./bilin-cli" ] || [ ! -f "./bilin-web" ]; then
    wget -O "bilin_last.zip" "https://github.com/AmashiroNatsuki250/Bilin/releases/download/${version}/bilin-linux-aarch64-${version}.zip"
    unzip -o bilin_last.zip -d ./
    mv ./bilin-linux-aarch64/* ./
    rm -rf ./bilin_last.zip
    rm -rf ./bilin-linux-aarch64
fi

zip -r -o -X "${filename}" ./ -x '.git/*' -x '.github/*' -x 'folder/*' -x 'build.sh' -x 'magisk_update.json'