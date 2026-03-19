# thorn-django

Django plugin for [Thorn](https://github.com/anthropics/thorn). Catches bugs, security issues, and performance problems in Django and DRF code by combining static AST analysis with live model introspection.

## Why?

Standard Python linters don't understand Django. They can't tell you that your serializer references a field that doesn't exist, that your `.filter()` uses a nonexistent lookup, or that your `select_related()` follows every FK chain in your database.

thorn-django can — it reads your actual model graph at runtime and cross-references it against your code.

## Quick Start

```sh
# Static checks only (no setup required)
thorn .

# With Django model introspection
thorn . --thorn-django-settings=myproject.settings

# Or pre-generate the graph (works in Docker)
python -m thorn_django --settings myproject.settings
thorn .
```

## 60+ Checks

### Static AST Checks (no setup required)

| Code | Issue |
|------|-------|
| DJ001 | `null=True` on string fields — use `blank=True` |
| DJ002 | `exclude` in ModelForm/Serializer Meta — use `fields` |
| DJ003 | `.raw()` or `.extra()` — prefer QuerySet methods |
| DJ006 | ForeignKey without `on_delete` |
| DJ007 | `fields = '__all__'` — new fields auto-exposed |
| DJ008 | `order_by('?')` — full table scan |
| DJ009 | QuerySet in boolean context — use `.exists()` |
| DJ011 | `self.field += N` race condition — use `F()` |
| DJ014 | SQL injection via string interpolation in `.raw()`/`.execute()` |
| DJ017 | `@csrf_exempt` on non-webhook view |
| DJ019 | `.count() > 0` — use `.exists()` |
| DJ020 | `select_related()` without arguments follows ALL FKs |
| DJ022 | Mutable default on JSONField |
| DJ026 | `.save()`/`.create()` in loop — use `bulk_create()` |
| DJ027 | Celery `.delay()` inside `transaction.atomic()` |
| DJ030 | DRF `AllowAny` or empty `permission_classes` |
| DJ032 | Django `ValidationError` in DRF code causes 500s |

### Model Graph Checks (with introspection)

| Code | Issue |
|------|-------|
| DJ101 | Model missing `__str__` |
| DJ102 | Duplicate `related_name` |
| DJ103 | `null=True` on string field (graph-validated) |

### Cross-Referencing Checks (AST + graph)

| Code | Issue |
|------|-------|
| DJ201 | Invalid field in `.filter()`/`.exclude()`/`.create()` |
| DJ202 | Invalid field in `.values()`/`.order_by()` |
| DJ205 | Serializer `Meta.fields` references nonexistent model field |
| DJ207 | `self.fk.id` triggers DB query — use `self.fk_id` |

### Settings & Security

| Code | Issue |
|------|-------|
| DJ012 | `DEBUG = True` in production |
| DJ013 | Missing `SECURE_SSL_REDIRECT`, `SESSION_COOKIE_SECURE`, etc. |
| DJ016 | Hardcoded `SECRET_KEY` |

### Dynamic Validation (runtime)

| Code | Issue |
|------|-------|
| DV001 | Missing `__str__` (MRO walk) |
| DV202 | Missing migrations |
| DV401 | Template syntax errors |
| DV501 | ModelForm field mismatches |
| DV601 | Unimportable dotted-path settings |

[Full check reference →](docs/checks.md)

## Check Levels

```sh
thorn . --check=fix      # Bugs + security only
thorn . --check=improve  # + Performance + deprecations (default)
thorn . --check=all      # + Style + complexity
```

## How It Works

```
┌─────────────────────────────────────────────────────┐
│                    thorn-django                      │
│                                                      │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────┐ │
│  │  AST Checks │  │ Graph Checks │  │  Cross     │ │
│  │  (per-file) │  │ (per-model)  │  │  Checks    │ │
│  │  DJ001-041  │  │  DJ101-104   │  │  DJ201-207 │ │
│  └──────┬──────┘  └──────┬───────┘  └─────┬──────┘ │
│         │                │                 │        │
│         │         ┌──────▼───────┐         │        │
│         │         │  Model Graph │◀────────┘        │
│         │         │  (AppGraph)  │                   │
│         │         └──────┬───────┘                   │
│         │                │                           │
│         │    ┌───────────┴──────────┐                │
│         │    │                      │                │
│         │  ┌─▼──────────┐  ┌───────▼──────┐         │
│         │  │ PyO3 Bridge│  │ JSON file    │         │
│         │  │ (in-proc)  │  │(.thorn/*.json)│        │
│         │  └────────────┘  └──────────────┘         │
│         │                                            │
└─────────┼────────────────────────────────────────────┘
          │
          ▼
   ┌──────────────┐
   │  thorn (CLI) │  ← generic linter engine
   └──────────────┘
```

1. **thorn** discovers Python files and dispatches to registered plugins
2. **thorn-django** parses each file's AST and runs 40+ static checks
3. If a model graph is available (via PyO3 or `.thorn/graph.json`), graph and cross-referencing checks also run
4. Dynamic validation runs Django's own check framework, migration detector, and template compiler

## Graph Generation

```sh
# Option 1: Auto-detect (if Django is importable)
thorn . --thorn-django-settings=myproject.settings

# Option 2: Pre-generate (for Docker / CI)
python -m thorn_django --settings myproject.settings
# Creates .thorn/graph.json

# Option 3: In Docker
docker compose exec app python -m thorn_django
```

## Building

```sh
# Run tests (requires Docker for integration tests)
docker compose up --build

# Or build standalone (requires Python 3.11+ dev headers)
cargo build --release
```

## License

MIT
