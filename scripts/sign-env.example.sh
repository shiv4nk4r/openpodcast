# Copy to sign-env.sh (gitignored) and fill in, then:
#   source scripts/sign-env.sh && npm run tauri -- build --bundles dmg
#
# Signing identity is already set in src-tauri/tauri.conf.json.
# Notarization needs ONE of the two auth methods below.

# --- Option A: App Store Connect API key (recommended) ---
# Create at: https://appstoreconnect.apple.com/access/integrations/api
# export APPLE_API_ISSUER="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
# export APPLE_API_KEY="XXXXXXXXXX"
# export APPLE_API_KEY_PATH="$HOME/Downloads/AuthKey_XXXXXXXXXX.p8"

# --- Option B: Apple ID + app-specific password ---
# export APPLE_ID="your@email.com"
# export APPLE_PASSWORD="xxxx-xxxx-xxxx-xxxx"   # app-specific password
# export APPLE_TEAM_ID="NH6XX7B9AW"
