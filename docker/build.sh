#!/bin/bash

set -euo pipefail

if docker image inspect 'acme-azure-function-build' >/dev/null; then
    exit 0
fi

directory="$(mktemp -d)"
trap "rm -rf '$directory'" EXIT

>"$directory/Dockerfile" cat <<-EOF
FROM mcr.microsoft.com/dotnet/core/sdk:3.1

ARG user_id
ARG username

COPY run.sh /
RUN user_id=\${user_id} username=\${username} /run.sh
ENV DOTNET_CLI_TELEMETRY_OPTOUT 'true'
ENV PATH "$PATH:/usr/local/bin/func"

USER \$user_id
WORKDIR /home/\$username

CMD ["/bin/bash"]
EOF

>"$directory/run.sh" cat <<-EOF
#!/bin/bash

set -euo pipefail

apt-get update

apt-get install -y apt-transport-https lsb-release
curl -L https://packages.microsoft.com/keys/microsoft.asc | gpg --dearmor > /etc/apt/trusted.gpg.d/microsoft.asc.gpg
echo "deb [arch=amd64] https://packages.microsoft.com/repos/azure-cli/ \$(lsb_release -cs) main" > /etc/apt/sources.list.d/azure-cli.list
apt-get update

apt-get install -y azure-cli jq unzip

apt-get clean

mkdir -p /usr/local/bin/func/
curl -Lo /usr/local/bin/func/func.zip 'https://github.com/Azure/azure-functions-core-tools/releases/download/3.0.2245/Azure.Functions.Cli.linux-x64.3.0.2245.zip'
unzip -d /usr/local/bin/func/ /usr/local/bin/func/func.zip
rm /usr/local/bin/func/func.zip
chmod +x /usr/local/bin/func/func /usr/local/bin/func/gozip

useradd -u "\$user_id" -d "/home/\$username" -m "\$username"
EOF
chmod +x "$directory/run.sh"

docker build \
    -t 'acme-azure-function-build' \
    --build-arg "user_id=$(id -u)" \
    --build-arg "username=$(id -un)" \
    "$directory"
