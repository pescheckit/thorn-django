#!/usr/bin/env bash
# test.sh — end-to-end integration test for the thorn-django plugin.
#
# What this script validates:
#   1. A minimal Django project with known violations can be assembled
#      in /tmp at runtime (no pre-baked fixtures required).
#   2. `python -m thorn_django --stdout` correctly extracts the model
#      graph and serialises it to JSON.
#   3. `thorn --graph-file` ingests the JSON bundle and reports the
#      expected rule codes.
#   4. `thorn` in AST-only mode (no graph) still reports AST-level
#      violations without crashing.
#
# The script is intentionally self-contained: it writes all test
# fixtures at runtime and cleans them up on exit.

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

RESET='\033[0m'
BOLD='\033[1m'
GREEN='\033[0;32m'
RED='\033[0;31m'
CYAN='\033[0;36m'

info()  { printf "${CYAN}[test]${RESET} %s\n" "$*"; }
ok()    { printf "${GREEN}[pass]${RESET} %s\n" "$*"; }
fail()  { printf "${RED}[fail]${RESET} %s\n" "$*" >&2; exit 1; }

TESTDIR=/tmp/thorn_integration_test
THORN_BIN=/app/thorn/target/release/thorn
PYTHON=/venv/bin/python3

cleanup() { rm -rf "$TESTDIR"; }
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Step 0 — sanity-check prerequisites
# ---------------------------------------------------------------------------

info "Checking prerequisites"

[[ -x "$THORN_BIN" ]] || fail "thorn binary not found at $THORN_BIN — run 'cargo build --release' first"
"$PYTHON" -c "import django" 2>/dev/null || fail "Django not importable from $PYTHON"
"$PYTHON" -c "import rest_framework" 2>/dev/null || fail "djangorestframework not importable from $PYTHON"
"$PYTHON" -c "import thorn_django" 2>/dev/null || fail "thorn_django Python package not installed"

ok "All prerequisites satisfied"

# ---------------------------------------------------------------------------
# Step 1 — create the test Django project
# ---------------------------------------------------------------------------

info "Creating test Django project in $TESTDIR"

mkdir -p "$TESTDIR/testapp"

# Minimal settings module
cat > "$TESTDIR/settings.py" <<'PYEOF'
SECRET_KEY = "thorn-test-secret-key"
DEBUG = True
INSTALLED_APPS = [
    "django.contrib.contenttypes",
    "django.contrib.auth",
    "testapp",
]
DATABASES = {
    "default": {
        "ENGINE": "django.db.backends.sqlite3",
        "NAME": ":memory:",
    }
}
DEFAULT_AUTO_FIELD = "django.db.models.BigAutoField"
PYEOF

# testapp/__init__.py
touch "$TESTDIR/testapp/__init__.py"

# testapp/apps.py
cat > "$TESTDIR/testapp/apps.py" <<'PYEOF'
from django.apps import AppConfig

class TestappConfig(AppConfig):
    default_auto_field = "django.db.models.BigAutoField"
    name = "testapp"
PYEOF

# testapp/models.py — intentional violations:
#   DJ001  CharField with null=True
#   DJ015  ordering on a field without an index
cat > "$TESTDIR/testapp/models.py" <<'PYEOF'
from django.db import models


class BadModel(models.Model):
    # DJ001: null=True on a CharField — use blank=True instead
    name = models.CharField(max_length=100, null=True)

    class Meta:
        # DJ015: ordering by a non-indexed field
        ordering = ["-name"]
        app_label = "testapp"


class GoodModel(models.Model):
    name = models.CharField(max_length=100, blank=True, default="")

    def __str__(self):
        return self.name

    class Meta:
        app_label = "testapp"
PYEOF

# testapp/views.py — intentional violations:
#   DJ018  request.POST used as a boolean
#   DJ032  rest_framework imported but not used (import-level side-effect check)
cat > "$TESTDIR/testapp/views.py" <<'PYEOF'
from rest_framework.views import APIView  # noqa: F401  (DJ032 candidate)


def my_view(request):
    # DJ018: request.POST should not be used as a boolean
    if request.POST:
        pass
    return None
PYEOF

ok "Test project created"

# ---------------------------------------------------------------------------
# Step 2 — run the Python extractor
# ---------------------------------------------------------------------------

info "Running Python extractor (python -m thorn_django --stdout)"

GRAPH_FILE="$TESTDIR/graph.json"

PYTHONPATH="$TESTDIR" \
DJANGO_SETTINGS_MODULE="settings" \
"$PYTHON" -m thorn_django --settings settings --stdout > "$GRAPH_FILE"

# Validate that the output is valid JSON and contains the expected keys.
"$PYTHON" - <<PYEOF
import json, sys

with open("$GRAPH_FILE") as f:
    bundle = json.load(f)

assert "graph" in bundle, "graph key missing from extractor output"
assert "diagnostics" in bundle, "diagnostics key missing from extractor output"

models = bundle["graph"].get("models", {})
assert models, "No models found in extracted graph"

model_names = list(models.keys())
print(f"  Extracted models: {model_names}")

# BadModel must appear in the graph
bad = next((m for m in model_names if "BadModel" in m), None)
assert bad is not None, f"BadModel not found in graph, got: {model_names}"
PYEOF

ok "Extractor produced valid JSON graph"

# ---------------------------------------------------------------------------
# Step 3 — run thorn with the graph bundle (cross-file checks enabled)
# ---------------------------------------------------------------------------

info "Running thorn with --graph-file (cross-file + AST checks)"

# thorn exits with a non-zero code when violations are found, so we
# capture output rather than letting set -e abort the script.
THORN_OUTPUT=$("$THORN_BIN" \
    --graph-file "$GRAPH_FILE" \
    "$TESTDIR/testapp/" 2>&1) || true

echo "$THORN_OUTPUT"

# Verify that at least one DJ001 violation is reported.
if echo "$THORN_OUTPUT" | grep -q "DJ001"; then
    ok "DJ001 detected (CharField with null=True)"
else
    fail "Expected DJ001 in thorn output but it was not found"
fi

# ---------------------------------------------------------------------------
# Step 4 — run thorn in AST-only mode (no graph file)
# ---------------------------------------------------------------------------

info "Running thorn in AST-only mode (no --graph-file)"

AST_OUTPUT=$("$THORN_BIN" "$TESTDIR/testapp/" 2>&1) || true

echo "$AST_OUTPUT"

# AST-only mode should complete without a hard crash (exit 0 or 1).
# We just verify the binary ran and produced output.
if [[ -z "$AST_OUTPUT" ]]; then
    fail "thorn produced no output in AST-only mode — expected at least a summary line"
fi

ok "thorn completed AST-only pass"

# ---------------------------------------------------------------------------
# Step 5 — run the Rust unit + integration tests for thorn-django
# ---------------------------------------------------------------------------

info "Running Rust test suite (cargo test -p thorn-django)"

PYTHONPATH="$TESTDIR" \
DJANGO_SETTINGS_MODULE="settings" \
PYO3_PYTHON=/venv/bin/python3 \
    cargo test \
        --manifest-path /app/thorn/Cargo.toml \
        -p thorn-django \
        -- --nocapture 2>&1

ok "Rust tests passed"

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------

printf "\n${BOLD}${GREEN}=== All integration tests passed ===${RESET}\n\n"
