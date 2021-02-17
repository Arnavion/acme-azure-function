#!/bin/bash

set -euo pipefail

uid="$(id -u)"
username="$(id -un)"

if ! docker image inspect 'azure-function-build-rust' >/dev/null; then
    (
        directory="$(mktemp -d)"
        trap "rm -rf '$directory'" EXIT

        >"$directory/Dockerfile" cat <<-EOF
FROM alpine

COPY build.sh /
COPY rust-toolchain /
RUN /build.sh
ENV PATH "\$PATH:$HOME/.cargo/bin"

CMD ["/bin/sh"]
EOF

        >"$directory/build.sh" cat <<-EOF
#!/bin/sh

set -euo pipefail

apk add --no-cache curl gcc libc-dev make sudo

adduser -u '$uid' -h '$HOME' -D '$username'

sudo -u '$username' mkdir -p '$HOME/.cargo/bin'

sudo -u '$username' curl -Lo '$HOME/.cargo/bin/rustup' 'https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-musl/rustup-init'
sudo -u '$username' chmod +x '$HOME/.cargo/bin/rustup'

sudo -u '$username' sh -c 'export PATH="\$PATH:$HOME/.cargo/bin"; rustup self update && rustup set profile minimal && cd / && rustc -vV'

rm -f /rust-toolchain

apk del curl sudo
EOF
        chmod +x "$directory/build.sh"

        cp ./rust-toolchain "$directory/"

        docker build \
            -t 'azure-function-build-rust' \
            "$directory"
    ) & :
fi

if ! docker image inspect 'azure-function-build-func' >/dev/null; then
    (
        directory="$(mktemp -d)"
        trap "rm -rf '$directory'" EXIT

        >"$directory/Dockerfile" cat <<-EOF
FROM debian:10-slim

COPY build.sh /
RUN /build.sh
ENV PATH "\$PATH:/usr/local/bin/func"

CMD ["/bin/bash"]
EOF

        >"$directory/build.sh" cat <<-EOF
#!/bin/bash

set -euo pipefail

apt-get update -y

apt-get install -y apt-transport-https curl gpg libicu63 lsb-release unzip
curl -L 'https://packages.microsoft.com/keys/microsoft.asc' | gpg --dearmor > /etc/apt/trusted.gpg.d/microsoft.asc.gpg
echo "deb [arch=amd64] https://packages.microsoft.com/repos/azure-cli/ \$(lsb_release -cs) main" > /etc/apt/sources.list.d/azure-cli.list
apt-get update -y

apt-get install -y azure-cli

useradd -u '$uid' -d '$HOME' -m '$username'

mkdir -p '/usr/local/bin/func/'
curl -Lo '/usr/local/bin/func/func.zip' 'https://github.com/Azure/azure-functions-core-tools/releases/download/3.0.3233/Azure.Functions.Cli.linux-x64.3.0.3233.zip'
unzip -d '/usr/local/bin/func/' '/usr/local/bin/func/func.zip'
rm '/usr/local/bin/func/func.zip'
chmod +x '/usr/local/bin/func/func' '/usr/local/bin/func/gozip'

apt-get remove -y --purge --autoremove curl gpg lsb-release unzip

apt-get clean -y
EOF
        chmod +x "$directory/build.sh"

        docker build \
            -t 'azure-function-build-func' \
            "$directory"
    ) & :
fi

wait $(jobs -pr)
