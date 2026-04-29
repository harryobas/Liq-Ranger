#!/usr/bin/env bash
set -euo pipefail

echo "🚀 Starting deployment on Hetzner VPS..."

# --- Validate required environment variables ---
REQUIRED_VARS=("PRIVATE_KEY" "DOCKER_IMAGE" "RPC_URL" "RPC_URL_HTTP" "GRAFANA_PASSWORD")
for var in "${REQUIRED_VARS[@]}"; do
    if [[ -z "${!var:-}" ]]; then
        echo "❌ $var environment variable is not set"
        exit 1
    fi
done
echo "✅ All required environment variables are set"

# --- Ensure data directory permissions and SQLite file ---
mkdir -p data/sled_db
chown -R 1000:1000 data
find data -type d -exec chmod 755 {} \;
chmod 700 data/sled_db
touch data/history.db && chmod 644 data/history.db

# --- Pull latest images ---
echo "🐳 Pulling images: $DOCKER_IMAGE and ${GRAFANA_IMAGE:-grafana/grafana:latest}"
docker compose pull

# --- Stop old containers ---
echo "🧹 Stopping old containers..."
docker compose down --remove-orphans --timeout 30 || true

# --- Start all services ---
echo "🚀 Starting containers..."
if ! docker compose up -d; then
    echo "❌ Failed to start containers"
    docker compose logs
    exit 1
fi

# --- Wait for SQLite file to be created (bot may need a few seconds) ---
echo "⏳ Waiting for SQLite database to appear..."
for i in {1..30}; do
    if [[ -f "data/history.db" ]]; then
        echo "✅ SQLite database found"
        break
    fi
    sleep 1
done

# Re-ensure permissions in case bot recreated the file
if [[ -f "data/history.db" ]]; then
    chmod 644 data/history.db
fi

# --- Health checks for both services ---
echo "📊 Checking service status..."

# Check bot (liq-ranger)
if docker compose ps -q liq-ranger &>/dev/null; then
    BOT_STATUS=$(docker inspect -f '{{.State.Status}}' $(docker compose ps -q liq-ranger))
    if [[ "$BOT_STATUS" == "running" ]]; then
        echo "✅ Bot container is running"
        BOT_ID=$(docker compose ps -q liq-ranger)
        if docker exec "$BOT_ID" printenv RPC_URL >/dev/null 2>&1; then
            echo "✅ RPC_URL is set in bot container"
        else
            echo "⚠️ RPC_URL not found in bot environment"
        fi
    else
        echo "❌ Bot container not running (status: $BOT_STATUS)"
        docker compose logs liq-ranger --tail=30
        exit 1
    fi
else
    echo "❌ Bot container not found"
    exit 1
fi

# Check Grafana
if docker compose ps -q ranger-grafana &>/dev/null; then
    GRAFANA_STATUS=$(docker inspect -f '{{.State.Status}}' $(docker compose ps -q ranger-grafana))
    if [[ "$GRAFANA_STATUS" == "running" ]]; then
        echo "✅ Grafana container is running"
        # Optional: test Grafana API health endpoint
        if curl -s -f http://localhost:3000/api/health >/dev/null 2>&1; then
            echo "✅ Grafana health endpoint responds"
        else
            echo "⚠️ Grafana health endpoint not responding yet (still starting?)"
        fi
    else
        echo "❌ Grafana container not running (status: $GRAFANA_STATUS)"
        docker compose logs ranger-grafana --tail=30
        exit 1
    fi
else
    echo "❌ Grafana container not found"
    exit 1
fi

# --- Prune old images to save disk space ---
echo "🧹 Pruning old Docker images..."
docker image prune -f

# --- Final output ---
echo "🎉 Deployment complete!"
echo "📊 Bot logs:     docker compose logs -f liq-ranger"
echo "📈 Grafana URL:  http://$(curl -s ifconfig.me):3000 (login: admin / \$GRAFANA_PASSWORD)"
echo "💡 To stop:      docker compose down"