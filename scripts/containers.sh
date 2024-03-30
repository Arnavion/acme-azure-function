#!/bin/bash

set -euo pipefail

uid="$(id -u)"
username="$(id -un)"

if ! podman image exists localhost/azure-function-build-rust; then
    (
        directory="$(mktemp -d)"
        trap "rm -rf '$directory'" EXIT

        >"$directory/Containerfile" cat <<-EOF
FROM docker.io/library/alpine

COPY build.sh /
COPY rust-toolchain.toml /
RUN /build.sh
ENV PATH "\$PATH:$HOME/.cargo/bin"
USER $uid

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

rm -f /rust-toolchain.toml

apk del curl sudo
EOF
        chmod +x "$directory/build.sh"

        cp ./rust-toolchain.toml "$directory/"

        podman image build \
            --layers=false \
            --tag=localhost/azure-function-build-rust \
            "$directory"
        podman image rm docker.io/library/alpine || :
    ) & :
fi

if ! podman image exists localhost/azure-function-build-func >/dev/null; then
    (
        directory="$(mktemp -d)"
        trap "rm -rf '$directory'" EXIT

        >"$directory/Containerfile" cat <<-EOF
FROM docker.io/library/debian:12-slim

COPY build.sh /
RUN /build.sh
ENV PATH "\$PATH:/usr/local/bin/func"
USER $uid

CMD ["/bin/bash"]
EOF

        >"$directory/build.sh" cat <<-EOF
#!/bin/bash

set -euo pipefail

apt-get update -y

apt-get install -y apt-transport-https curl gpg libicu72 lsb-release unzip
curl -L 'https://packages.microsoft.com/keys/microsoft.asc' | gpg --dearmor > /etc/apt/trusted.gpg.d/microsoft.asc.gpg
echo "deb [arch=amd64] https://packages.microsoft.com/repos/azure-cli/ \$(lsb_release -cs) main" > /etc/apt/sources.list.d/azure-cli.list
apt-get update -y

apt-get install -y azure-cli

useradd -u '$uid' -d '$HOME' -m '$username'

mkdir -p '/usr/local/bin/func/'
# https://github.com/Azure/azure-functions-core-tools/releases/latest
curl -Lo '/usr/local/bin/func/func.zip' 'https://github.com/Azure/azure-functions-core-tools/releases/download/4.0.5611/Azure.Functions.Cli.linux-x64.4.0.5611.zip'
unzip -d '/usr/local/bin/func/' '/usr/local/bin/func/func.zip'
rm '/usr/local/bin/func/func.zip'
chmod +x '/usr/local/bin/func/func' '/usr/local/bin/func/gozip'

apt-get remove -y --purge --autoremove curl gpg lsb-release unzip

apt-get clean -y
EOF
        chmod +x "$directory/build.sh"

        podman image build \
            --layers=false \
            --tag=localhost/azure-function-build-func \
            "$directory"
        podman image rm docker.io/library/debian:12-slim || :
    ) & :
fi

wait $(jobs -pr)
