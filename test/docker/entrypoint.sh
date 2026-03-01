#!/bin/bash
# Entrypoint script for E2E test container
#
# This script:
# 1. Starts the Docker daemon in the background
# 2. Waits for Docker to be ready
# 3. Configures DevPod
# 4. Runs pytest with E2E tests

set -e

echo "Starting Docker daemon..."

# Start Docker daemon in background
# The dind image uses dockerd-entrypoint.sh to start dockerd
dockerd-entrypoint.sh &

# Wait for Docker to be ready
echo "Waiting for Docker to be ready..."
timeout=60
while ! docker info >/dev/null 2>&1; do
    sleep 1
    timeout=$((timeout - 1))
    if [ $timeout -le 0 ]; then
        echo "ERROR: Docker daemon failed to start within 60 seconds"
        exit 1
    fi
done
echo "Docker is ready!"

# Show Docker info
docker info --format '{{.ServerVersion}}' | xargs echo "Docker version:"

# Configure DevPod to use Docker provider
echo "Configuring DevPod..."
devpod provider add docker 2>/dev/null || true
devpod provider use docker

# Run E2E tests
echo "Running E2E tests..."
cd /app

# Pass all arguments to pytest
# Override the default addopts which excludes e2e tests
exec pytest -m e2e test/e2e/ "$@"
