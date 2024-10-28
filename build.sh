#!/bin/bash

SCRIPT_DIR=$(cd $(dirname $0); pwd)

CERTS_DIR=`pwd`/certs
IMAGES_DIR=`pwd`/images
RUN_DIR=`pwd`/run
IMAGE_NAME=dstack-0.1.0
IMAGE_TMP_DIR=`pwd`/tmp/images/$IMAGE_NAME
CERBOT_WORKDIR=$RUN_DIR/certbot

CONFIG_FILE=$SCRIPT_DIR/build-config.sh
if [ -f $CONFIG_FILE ]; then
    source $CONFIG_FILE
else
    cat <<EOF > $CONFIG_FILE
# base domain of kms rpc and tproxy rpc
# 1022.kvin.wang resolves to 10.0.2.2 which is host ip at the
# cvm point of view
BASE_DOMAIN=1022.kvin.wang

# kms and tproxy rpc listen port
TEEPOD_RPC_LISTEN_PORT=9080
# CIDs allocated to VMs start from this number of type unsigned int32
TEEPOD_CID_POOL_START=10000
# CID pool size
TEEPOD_CID_POOL_SIZE=1000

KMS_RPC_LISTEN_PORT=9043
TPROXY_RPC_LISTEN_PORT=9010

TPROXY_WG_INTERFACE=tproxy-$USER
TPROXY_WG_LISTEN_PORT=9182
TPROXY_WG_IP=10.0.3.1
TPROXY_WG_CLIENT_IP_RANGE=10.0.3.0/24

TPROXY_LISTEN_PORT1=9443
TPROXY_LISTEN_PORT2=9090

TPROXY_PUBLIC_DOMAIN=app.kvin.wang
TPROXY_CERT=/etc/rproxy/certs/cert.pem
TPROXY_KEY=/etc/rproxy/certs/key.pem

# for certbot
CF_API_TOKEN=
CF_ZONE_ID=
ACME_URL=https://acme-staging-v02.api.letsencrypt.org/directory
EOF
    echo "Config file $CONFIG_FILE created, please edit it to configure the build"
    exit 1
fi

TPROXY_WG_KEY=$(wg genkey)
TPROXY_WG_PUBKEY=$(echo $TPROXY_WG_KEY | wg pubkey)

# Step 1: build binaries

cargo build --release
cp ../target/release/{tproxy,kms,teepod,certbot,ct_monitor} .

# Step 2: build guest images
make -C ../../ dist DIST_DIR=$IMAGE_TMP_DIR

make_image_dist() {
    local img_name=$1
    local rootfs_name=$2
    local img_dist_dir=$IMAGES_DIR/$img_name
    local rootfs_hash

    mkdir -p $img_dist_dir
    rootfs_hash=$(sha256sum "$IMAGE_TMP_DIR/$rootfs_name.cpio" | awk '{print $1}')
    cat <<EOF > $img_dist_dir/metadata.json
{
    "bios": "ovmf.fd",
    "kernel": "bzImage",
    "cmdline": "console=ttyS0 init=/init dstack.fde=1 dstack.integrity=0",
    "initrd": "initramfs.cpio.gz",
    "rootfs": "rootfs.iso",
    "rootfs_hash": "$rootfs_hash"
}
EOF

    cp $IMAGE_TMP_DIR/ovmf.fd $img_dist_dir/
    cp $IMAGE_TMP_DIR/bzImage $img_dist_dir/
    cp $IMAGE_TMP_DIR/initramfs.cpio.gz $img_dist_dir/
    cp $IMAGE_TMP_DIR/$rootfs_name.iso $img_dist_dir/rootfs.iso
}

make_image_dist dstack-0.1.0 rootfs
make_image_dist dstack-0.1.0-dev rootfs-dev

# Step 3: make certs
make -C .. certs DOMAIN=$BASE_DOMAIN TO=$CERTS_DIR

# Step 4: generate config files

# kms
cat <<EOF > kms.toml
log_level = "info"
address = "0.0.0.0"
port = $KMS_RPC_LISTEN_PORT

[tls]
key = "$CERTS_DIR/kms-rpc.key"
certs = "$CERTS_DIR/kms-rpc.cert"

[tls.mutual]
ca_certs = "$CERTS_DIR/tmp-ca.cert"
mandatory = false

[core]
root_ca_cert = "$CERTS_DIR/root-ca.cert"
root_ca_key = "$CERTS_DIR/root-ca.key"
subject_postfix = ".phala"

[core.allowed_mr]
allow_all = true
mrtd = []
rtmr0 = []
rtmr1 = []
rtmr2 = []
EOF

# tproxy
cat <<EOF > tproxy.toml
log_level = "info"
address = "0.0.0.0"
port = $TPROXY_RPC_LISTEN_PORT

[tls]
key = "$CERTS_DIR/tproxy-rpc.key"
certs = "$CERTS_DIR/tproxy-rpc.cert"

[tls.mutual]
ca_certs = "$CERTS_DIR/root-ca.cert"
mandatory = false

[core.certbot]
workdir = "$CERBOT_WORKDIR"

[core.wg]
private_key = "$TPROXY_WG_KEY"
public_key = "$TPROXY_WG_PUBKEY"
ip = "$TPROXY_WG_IP"
listen_port = $TPROXY_WG_LISTEN_PORT
client_ip_range = "$TPROXY_WG_CLIENT_IP_RANGE"
config_path = "$RUN_DIR/wg.conf"
interface = "$TPROXY_WG_INTERFACE"
endpoint = "10.0.2.2:$TPROXY_WG_LISTEN_PORT"

[core.proxy]
cert_chain = "$TPROXY_CERT"
cert_key = "$TPROXY_KEY"
base_domain = "$TPROXY_PUBLIC_DOMAIN"
config_path = "$RUN_DIR/rproxy.yaml"
portmap = [
    { listen_addr = "0.0.0.0", listen_port = $TPROXY_LISTEN_PORT1, target_port = 8080 },
    { listen_addr = "0.0.0.0", listen_port = $TPROXY_LISTEN_PORT2, target_port = 8090 },
]
EOF

# teepod
cat <<EOF > teepod.toml
log_level = "info"
port = $TEEPOD_RPC_LISTEN_PORT
image_path = "$IMAGES_DIR"
run_path = "$RUN_DIR/vm"

[cvm]
ca_cert = "$CERTS_DIR/root-ca.cert"
tmp_ca_cert = "$CERTS_DIR/tmp-ca.cert"
tmp_ca_key = "$CERTS_DIR/tmp-ca.key"
kms_url = "https://kms.$BASE_DOMAIN:$KMS_RPC_LISTEN_PORT"
tproxy_url = "https://tproxy.$BASE_DOMAIN:$TPROXY_RPC_LISTEN_PORT"
cid_start = $TEEPOD_CID_POOL_START
cid_pool_size = $TEEPOD_CID_POOL_SIZE

[gateway]
base_domain = "$TPROXY_PUBLIC_DOMAIN"
port = $TPROXY_LISTEN_PORT2
EOF

cat <<EOF > certbot.toml
# Path to the working directory
workdir = "$CERBOT_WORKDIR"
# ACME server URL
acme_url = "$ACME_URL"
# Cloudflare API token
cf_api_token = "$CF_API_TOKEN"
# Cloudflare zone ID
cf_zone_id = "$CF_ZONE_ID"
# Auto set CAA record
auto_set_caa = true
# Domain to issue certificates for
domain = "*.$TPROXY_PUBLIC_DOMAIN"
# Renew interval in seconds
renew_interval = 3600
# Number of days before expiration to trigger renewal
renew_days_before = 10
# Renew timeout in seconds
renew_timeout = 120
EOF

# Step 5: prepare run dir
mkdir -p $RUN_DIR
mkdir -p $CERBOT_WORKDIR/backup/preinstalled

# Step 6: setup wireguard interface
# Check if the WireGuard interface exists
if ! ip link show $TPROXY_WG_INTERFACE &> /dev/null; then
    sudo ip link add $TPROXY_WG_INTERFACE type wireguard
    sudo ip address add $TPROXY_WG_IP/24 dev $TPROXY_WG_INTERFACE
    sudo ip link set $TPROXY_WG_INTERFACE up
    echo "created and configured WireGuard interface $TPROXY_WG_INTERFACE"
else
    echo "WireGuard interface $TPROXY_WG_INTERFACE already exists"
fi
# sudo ip route add $TPROXY_WG_CLIENT_IP_RANGE dev $TPROXY_WG_INTERFACE

# Step 7: start services

# ./kms -c kms.toml
# ./certbot init -c certbot.toml
# sudo ./tproxy -c tproxy.toml
# ./teepod -c teepod.toml
