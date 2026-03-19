"""Extract Django model graph and run dynamic validation checks.

Usage:
    python -m thorn_django                                      # writes .thorn/graph.json
    python -m thorn_django --settings myproject.settings         # specify settings
    python -m thorn_django --stdout                              # output to stdout
    docker compose exec app python -m thorn_django               # in Docker
"""
import argparse
import json
import logging
import os
import sys


def main():
    parser = argparse.ArgumentParser(description="Extract Django model graph for Thorn")
    parser.add_argument("--settings", default=os.environ.get("DJANGO_SETTINGS_MODULE"),
                        help="Django settings module")
    parser.add_argument("--stdout", action="store_true",
                        help="Write JSON to stdout instead of .thorn/graph.json")
    args = parser.parse_args()

    if args.settings:
        os.environ["DJANGO_SETTINGS_MODULE"] = args.settings
    if not os.environ.get("DJANGO_SETTINGS_MODULE"):
        print("Error: DJANGO_SETTINGS_MODULE not set.", file=sys.stderr)
        sys.exit(1)

    logging.disable(logging.WARNING)
    import django
    django.setup()
    logging.disable(logging.NOTSET)

    graph = extract_graph()
    diagnostics = run_dynamic_checks()
    output = {"graph": graph, "diagnostics": diagnostics}

    if args.stdout:
        json.dump(output, sys.stdout)
    else:
        thorn_dir = os.path.join(os.getcwd(), ".thorn")
        os.makedirs(thorn_dir, exist_ok=True)
        graph_path = os.path.join(thorn_dir, "graph.json")
        with open(graph_path, "w") as f:
            json.dump(output, f)
        print(f"Wrote {len(graph['models'])} models + {len(diagnostics)} diagnostics to {graph_path}", file=sys.stderr)


def extract_graph():
    from django.apps import apps
    from django.conf import settings
    models = [_extract_model(m) for m in apps.get_models()]
    return {
        "models": models,
        "installed_apps": list(apps.app_configs.keys()),
        "settings": {
            "auth_user_model": getattr(settings, "AUTH_USER_MODEL", "auth.User"),
            "databases": list(getattr(settings, "DATABASES", {}).keys()),
            "middleware": list(getattr(settings, "MIDDLEWARE", [])),
            "extra": {},
        },
    }


def _extract_model(model):
    import inspect
    meta = model._meta
    try:
        source_file = os.path.relpath(inspect.getfile(model))
    except (TypeError, OSError):
        source_file = ""
    return {
        "app_label": meta.app_label, "name": model.__name__,
        "db_table": meta.db_table, "module": model.__module__,
        "source_file": source_file, "abstract_model": meta.abstract,
        "proxy": meta.proxy,
        "fields": _extract_fields(meta),
        "relations": _extract_relations(meta),
        "managers": _extract_managers(meta),
        "parents": [p.__name__ for p in meta.parents],
        "methods": [n for n in dir(model) if not n.startswith("_") and callable(getattr(model, n, None))],
    }


def _extract_fields(meta):
    fields = []
    for f in meta.local_concrete_fields:
        fc = f.__class__
        fclass = f"{fc.__module__}.{fc.__name__}"
        if any(x in fclass for x in ("ForeignKey", "OneToOne", "ManyToMany", "Rel")):
            continue
        from django.db.models.fields import NOT_PROVIDED
        default = None
        if f.default is not NOT_PROVIDED:
            default = f"<callable: {getattr(f.default, '__name__', '?')}>" if callable(f.default) else str(f.default)
        fields.append({
            "name": f.name, "column": f.column, "field_class": fclass,
            "native_type": _field_type(fclass), "nullable": f.null, "blank": f.blank,
            "default": default, "max_length": getattr(f, "max_length", None),
            "choices": [(str(c[0]), str(c[1])) for c in (f.choices or [])],
            "validators": [f"{v.__class__.__module__}.{v.__class__.__name__}" for v in f.validators],
            "primary_key": f.primary_key, "unique": f.unique, "db_index": f.db_index,
        })
    return fields


def _extract_relations(meta):
    relations = []
    for f in meta.get_fields(include_hidden=True):
        fc = f.__class__
        fclass = f"{fc.__module__}.{fc.__name__}"
        kind = None
        for k, v in {"ForeignKey": "ForeignKey", "OneToOneField": "OneToOne", "ManyToManyField": "ManyToMany", "ManyToOneRel": "Reverse", "OneToOneRel": "ReverseOneToOne", "ManyToManyRel": "ManyToMany"}.items():
            if k in fclass: kind = v; break
        if not kind: continue
        rm = getattr(f, "related_model", None)
        if rm is None: continue
        on_delete = None
        rf = getattr(f, "remote_field", None)
        if rf and hasattr(rf, "on_delete") and rf.on_delete:
            on_delete = rf.on_delete.__name__
        relations.append({
            "name": getattr(f, "name", "") or "", "kind": kind,
            "to_model": rm.__name__, "to_model_app": rm._meta.app_label,
            "related_name": getattr(f, "related_name", "") or "",
            "related_query_name": "", "on_delete": on_delete,
            "nullable": getattr(f, "null", False), "through_model": None,
        })
    return relations


def _extract_managers(meta):
    return [{
        "name": m.name,
        "manager_class": f"{m.__class__.__module__}.{m.__class__.__name__}",
        "queryset_class": f"{m._queryset_class.__module__}.{m._queryset_class.__name__}",
        "is_default": m == meta.default_manager,
        "custom_methods": [n for n in dir(m) if not n.startswith("_") and callable(getattr(m, n, None))],
    } for m in meta.managers]


def _field_type(fc):
    for k, v in {"IntegerField": "int", "AutoField": "int", "FloatField": "float", "DecimalField": "float", "BooleanField": "bool", "DateTimeField": "datetime", "DateField": "date", "TimeField": "time", "UUIDField": "UUID", "JSONField": "Any", "BinaryField": "bytes"}.items():
        if k in fc: return v
    return "str"


# ── Dynamic validation checks ─────────────────────────────────────────────

def run_dynamic_checks():
    d = []
    d.extend(_check_django_system_checks())
    d.extend(_check_models())
    d.extend(_check_serializers())
    d.extend(_check_migrations())
    d.extend(_check_missing_migrations())
    d.extend(_check_templates())
    d.extend(_check_dotted_path_settings())
    d.extend(_check_forms())
    return d


def _check_django_system_checks():
    from django.core import checks
    diags = []
    for c in checks.run_checks(include_deployment_checks=True):
        if c.level < checks.WARNING: continue
        code = {checks.WARNING: "DV-WARN", checks.ERROR: "DV-ERR", checks.CRITICAL: "DV-CRIT"}.get(c.level, "DV-WARN")
        diags.append({"code": code, "message": f"[{c.id}] {c.msg}", "range": None, "filename": "django.checks", "line": None, "col": None})
    return diags


def _is_third_party(path_or_module):
    """Check if a path or module name belongs to a third-party package."""
    s = str(path_or_module)
    return 'site-packages' in s or 'dist-packages' in s or '/venv/' in s or '/.venv/' in s


def _check_models():
    import inspect
    from django.apps import apps
    diags = []
    for model in apps.get_models():
        meta = model._meta
        if meta.abstract: continue
        name = f"{meta.app_label}.{model.__name__}"
        try: sf, sl = os.path.relpath(inspect.getfile(model)), inspect.getsourcelines(model)[1]
        except: sf, sl = model.__module__, None
        # Skip third-party models — developers can't fix these
        if _is_third_party(sf): continue
        has_str = any("__str__" in cls.__dict__ for cls in model.__mro__ if cls.__name__ not in ("Model", "object"))
        if not has_str:
            diags.append({"code": "DV001", "message": f"Model '{name}' has no __str__.", "range": None, "filename": sf, "line": sl, "col": None})
    return diags


def _check_serializers():
    try: from rest_framework.serializers import ModelSerializer
    except ImportError: return []
    diags = []
    for cls in _all_subclasses(ModelSerializer):
        meta = getattr(cls, "Meta", None)
        if not meta: continue
        model = getattr(meta, "model", None)
        if not model: continue
        fields = getattr(meta, "fields", None)
        if not fields or fields == "__all__": continue
        if not isinstance(fields, (list, tuple)): continue
        declared = getattr(cls, "_declared_fields", {})
        for fn in fields:
            try: model._meta.get_field(fn); continue
            except: pass
            if fn in declared: continue
            if hasattr(model, fn): continue
            diags.append({"code": "DV101", "message": f"Serializer '{cls.__name__}' field '{fn}' doesn't exist on '{model.__name__}'.", "range": None, "filename": cls.__module__, "line": None, "col": None})
    return diags


def _check_migrations():
    from django.db import connection
    from django.db.migrations.loader import MigrationLoader
    diags = []
    try:
        loader = MigrationLoader(connection, ignore_no_migrations=True)
        for app, migs in loader.detect_conflicts().items():
            diags.append({"code": "DV201", "message": f"Migration conflict in '{app}': {', '.join(migs)}", "range": None, "filename": "migrations", "line": None, "col": None})
    except Exception: pass
    return diags


def _check_missing_migrations():
    """DV202: Check if model changes exist without a corresponding migration."""
    from io import StringIO
    from django.core.management import call_command
    diags = []
    try:
        out = StringIO()
        call_command('makemigrations', '--check', '--dry-run', stdout=out, stderr=out)
    except SystemExit as e:
        if e.code != 0:
            output = out.getvalue().strip()
            diags.append({
                "code": "DV202", "range": None, "line": None, "col": None,
                "message": f"Missing migrations detected: {output}",
                "filename": "migrations",
            })
    except Exception:
        pass
    return diags


def _check_templates():
    """DV401: Compile all templates to catch TemplateSyntaxError."""
    from django.template import TemplateSyntaxError
    from django.template.loader import get_template
    from django.apps import apps
    diags = []
    template_dirs = []
    try:
        from django.conf import settings
        for engine_config in settings.TEMPLATES:
            template_dirs.extend(engine_config.get('DIRS', []))
        for app_config in apps.get_app_configs():
            app_template_dir = os.path.join(app_config.path, 'templates')
            if os.path.isdir(app_template_dir):
                template_dirs.append(app_template_dir)
    except Exception:
        return diags

    for tdir in template_dirs:
        if not os.path.isdir(tdir):
            continue
        # Skip third-party template directories
        if _is_third_party(tdir):
            continue
        for root, _, files in os.walk(tdir):
            for fname in files:
                if not fname.endswith(('.html', '.txt', '.xml', '.email')):
                    continue
                full_path = os.path.join(root, fname)
                rel_path = os.path.relpath(full_path, tdir)
                try:
                    get_template(rel_path)
                except TemplateSyntaxError as e:
                    diags.append({
                        "code": "DV401", "range": None, "line": None, "col": None,
                        "message": f"Template '{rel_path}' has a syntax error: {e}",
                        "filename": full_path,
                    })
                except Exception:
                    pass  # Template not found via loader, skip
    return diags


def _check_dotted_path_settings():
    """DV601: Verify dotted-path settings are importable."""
    from django.conf import settings
    from django.utils.module_loading import import_string
    diags = []

    # Settings that contain dotted import paths
    path_settings = {
        'MIDDLEWARE': getattr(settings, 'MIDDLEWARE', []),
        'AUTHENTICATION_BACKENDS': getattr(settings, 'AUTHENTICATION_BACKENDS', []),
        'PASSWORD_HASHERS': getattr(settings, 'PASSWORD_HASHERS', []),
    }

    # DRF settings
    drf_settings = getattr(settings, 'REST_FRAMEWORK', {})
    for key in ['DEFAULT_AUTHENTICATION_CLASSES', 'DEFAULT_PERMISSION_CLASSES',
                'DEFAULT_RENDERER_CLASSES', 'DEFAULT_THROTTLE_CLASSES',
                'DEFAULT_PAGINATION_CLASS', 'DEFAULT_FILTER_BACKENDS']:
        val = drf_settings.get(key)
        if val:
            if isinstance(val, str):
                path_settings[f'REST_FRAMEWORK.{key}'] = [val]
            elif isinstance(val, (list, tuple)):
                path_settings[f'REST_FRAMEWORK.{key}'] = list(val)

    for setting_name, paths in path_settings.items():
        for path in paths:
            if not isinstance(path, str) or '.' not in path:
                continue
            try:
                import_string(path)
            except ImportError as e:
                diags.append({
                    "code": "DV601", "range": None, "line": None, "col": None,
                    "message": f"Setting {setting_name} references '{path}' which cannot be imported: {e}",
                    "filename": "settings",
                })
    return diags


def _check_forms():
    """DV501: Validate ModelForm field definitions against models."""
    try:
        from django.forms import ModelForm
    except ImportError:
        return []
    diags = []
    for cls in _all_subclasses(ModelForm):
        # Skip third-party forms
        if _is_third_party(getattr(cls, '__module__', '')):
            continue
        meta = getattr(cls, 'Meta', None)
        if not meta:
            continue
        model = getattr(meta, 'model', None)
        if not model:
            continue
        fields = getattr(meta, 'fields', None)
        if not fields or fields == '__all__':
            continue
        if not isinstance(fields, (list, tuple)):
            continue
        for fn in fields:
            try:
                model._meta.get_field(fn)
            except Exception:
                if not hasattr(model, fn):
                    diags.append({
                        "code": "DV501", "range": None, "line": None, "col": None,
                        "message": f"ModelForm '{cls.__name__}' references field '{fn}' not on model '{model.__name__}'.",
                        "filename": cls.__module__,
                    })
    return diags


def _all_subclasses(cls, seen=None):
    if seen is None: seen = set()
    result = []
    for sc in cls.__subclasses__():
        if id(sc) in seen or sc.__name__.startswith("_"): continue
        seen.add(id(sc))
        meta = getattr(sc, "Meta", None)
        if meta and getattr(meta, "abstract", False): continue
        result.append(sc)
        result.extend(_all_subclasses(sc, seen))
    return result
