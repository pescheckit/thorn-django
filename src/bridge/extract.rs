use pyo3::prelude::*;
use pyo3::types::PyDict;
use thorn_api::graph::*;

/// Extract the full model graph from a booted Django process.
pub fn extract_graph(py: Python<'_>) -> PyResult<ModelGraph> {
    let apps = py.import("django.apps")?.getattr("apps")?;

    let models_list = apps.call_method0("get_models")?;
    let mut models = Vec::new();

    for model_cls in models_list.try_iter()? {
        let model_cls = model_cls?;
        match extract_model(py, &model_cls) {
            Ok(model) => models.push(model),
            Err(e) => {
                eprintln!("Warning: failed to extract model: {e}");
            }
        }
    }

    let installed_apps = extract_installed_apps(py)?;
    let settings = extract_settings(py)?;

    Ok(ModelGraph {
        models,
        installed_apps,
        settings,
    })
}

fn extract_model(py: Python<'_>, model_cls: &Bound<'_, PyAny>) -> PyResult<DjangoModel> {
    let meta = model_cls.getattr("_meta")?;

    let app_label: String = meta.getattr("app_label")?.extract()?;
    let model_name: String = meta.getattr("model_name")?.extract()?;
    let name = capitalize(&model_name);
    let db_table: String = meta.getattr("db_table")?.extract()?;
    let module: String = model_cls.getattr("__module__")?.extract()?;
    let abstract_model: bool = meta.getattr("abstract")?.extract()?;
    let proxy: bool = meta.getattr("proxy")?.extract()?;

    let parents = extract_parents(&meta)?;
    let fields = extract_fields(py, &meta)?;
    let relations = extract_relations(py, &meta)?;
    let managers = extract_managers(py, model_cls)?;
    let methods = extract_methods(py, model_cls)?;

    Ok(DjangoModel {
        app_label,
        name,
        db_table,
        module,
        source_file: String::new(),
        abstract_model,
        proxy,
        fields,
        relations,
        managers,
        parents,
        methods,
    })
}

fn extract_parents(meta: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let parents_dict = meta.getattr("parents")?;
    let mut parents = Vec::new();
    for parent in parents_dict.call_method0("keys")?.try_iter()? {
        let parent = parent?;
        let parent_name: String = parent.getattr("__name__")?.extract()?;
        parents.push(parent_name);
    }
    Ok(parents)
}

fn extract_fields(_py: Python<'_>, meta: &Bound<'_, PyAny>) -> PyResult<Vec<FieldDef>> {
    let mut fields = Vec::new();

    let concrete_fields = meta.getattr("local_concrete_fields")?;
    for field in concrete_fields.try_iter()? {
        let field = field?;
        let field_class = get_class_path(&field)?;

        // Skip relational fields — handled in extract_relations
        if is_relational(&field_class) {
            continue;
        }

        let name: String = field.getattr("name")?.extract()?;
        let column: String = field.getattr("column")?.extract()?;
        let nullable: bool = field.getattr("null")?.extract()?;
        let blank: bool = field.getattr("blank")?.extract()?;
        let primary_key: bool = field.getattr("primary_key")?.extract()?;
        let unique: bool = field.getattr("unique")?.extract()?;
        let db_index: bool = field.getattr("db_index")?.extract()?;

        let max_length = field
            .getattr("max_length")
            .ok()
            .and_then(|v| v.extract::<i64>().ok());

        let native_type = field_class_to_python_type(&field_class);

        let default = extract_default(_py, &field)?;
        let choices = extract_choices(&field)?;
        let validators = extract_field_validators(&field)?;

        fields.push(FieldDef {
            name,
            column,
            field_class,
            native_type,
            nullable,
            blank,
            default,
            max_length,
            choices,
            validators,
            primary_key,
            unique,
            db_index,
        });
    }

    Ok(fields)
}

fn extract_relations(_py: Python<'_>, meta: &Bound<'_, PyAny>) -> PyResult<Vec<RelationDef>> {
    let mut relations = Vec::new();

    let kwargs = PyDict::new(_py);
    kwargs.set_item("include_hidden", true)?;
    let all_fields = meta.call_method("get_fields", (), Some(&kwargs))?;

    for field in all_fields.try_iter()? {
        let field = field?;
        let field_class = get_class_path(&field)?;

        let kind = if field_class.contains("ForeignKey") {
            RelationKind::ForeignKey
        } else if field_class.contains("OneToOneField") {
            RelationKind::OneToOne
        } else if field_class.contains("ManyToManyField") {
            RelationKind::ManyToMany
        } else if field_class.contains("ManyToOneRel") {
            RelationKind::Reverse
        } else if field_class.contains("OneToOneRel") {
            RelationKind::ReverseOneToOne
        } else if field_class.contains("ManyToManyRel") {
            RelationKind::ManyToMany
        } else {
            continue;
        };

        let name: String = field
            .getattr("name")
            .or_else(|_| field.call_method0("get_accessor_name"))
            .and_then(|v| v.extract())
            .unwrap_or_default();

        let related_model = field.getattr("related_model").or_else(|_| {
            field
                .getattr("remote_field")
                .and_then(|rf| rf.getattr("model"))
        });

        let (to_model, to_model_app) = match related_model {
            Ok(rm) => {
                let to_model: String = rm.getattr("__name__")?.extract()?;
                let to_model_app: String = rm.getattr("_meta")?.getattr("app_label")?.extract()?;
                (to_model, to_model_app)
            }
            Err(_) => continue,
        };

        let related_name: String = field
            .getattr("related_name")
            .and_then(|v| v.extract())
            .unwrap_or_default();

        let related_query_name: String = field
            .call_method0("get_related_field")
            .and_then(|v| v.getattr("name"))
            .and_then(|v| v.extract())
            .unwrap_or_default();

        let on_delete: Option<String> = field
            .getattr("remote_field")
            .ok()
            .and_then(|rf| rf.getattr("on_delete").ok())
            .and_then(|od| od.getattr("__name__").ok())
            .and_then(|n| n.extract().ok());

        let nullable: bool = field
            .getattr("null")
            .and_then(|v| v.extract())
            .unwrap_or(false);

        let through_model: Option<String> = if kind == RelationKind::ManyToMany {
            field
                .getattr("remote_field")
                .ok()
                .and_then(|rf| rf.getattr("through").ok())
                .and_then(|t| t.getattr("__name__").ok())
                .and_then(|n| n.extract().ok())
        } else {
            None
        };

        relations.push(RelationDef {
            name,
            kind,
            to_model,
            to_model_app,
            related_name,
            related_query_name,
            on_delete,
            nullable,
            through_model,
        });
    }

    Ok(relations)
}

fn extract_managers(_py: Python<'_>, model_cls: &Bound<'_, PyAny>) -> PyResult<Vec<ManagerDef>> {
    let meta = model_cls.getattr("_meta")?;
    let managers = meta.getattr("managers")?;
    let default_manager = meta.getattr("default_manager")?;

    let mut result = Vec::new();

    for manager in managers.try_iter()? {
        let manager = manager?;
        let name: String = manager.getattr("name")?.extract()?;
        let is_default = manager.is(&default_manager);
        let manager_class = get_class_path(&manager)?;

        let queryset_class: String = manager
            .getattr("_queryset_class")
            .and_then(|qs| get_class_path(&qs))
            .unwrap_or_else(|_| "django.db.models.QuerySet".into());

        let custom_methods = extract_public_methods(_py, &manager)?;

        result.push(ManagerDef {
            name,
            manager_class,
            queryset_class,
            is_default,
            custom_methods,
        });
    }

    Ok(result)
}

fn extract_methods(py: Python<'_>, model_cls: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let mut methods = extract_public_methods(py, model_cls)?;
    // Also include @property descriptors — they act as virtual fields
    let builtins = py.import("builtins")?;
    let property_type = builtins.getattr("property")?;
    for cls in model_cls.getattr("__mro__")?.try_iter()? {
        let cls = cls?;
        let cls_name: String = cls.getattr("__name__")?.extract()?;
        if cls_name == "Model" || cls_name == "object" {
            break;
        }
        if let Ok(dict) = cls.getattr("__dict__") {
            if let Ok(items) = dict.call_method0("items") {
                for item in (items.try_iter()?).flatten() {
                    if let Ok((key, val)) = item.extract::<(String, pyo3::Bound<'_, pyo3::PyAny>)>()
                    {
                        if !key.starts_with('_')
                            && val.is_instance(&property_type).unwrap_or(false)
                            && !methods.contains(&key)
                        {
                            methods.push(key);
                        }
                    }
                }
            }
        }
    }
    Ok(methods)
}

fn extract_installed_apps(py: Python<'_>) -> PyResult<Vec<String>> {
    let settings = py.import("django.conf")?.getattr("settings")?;
    let apps: Vec<String> = settings.getattr("INSTALLED_APPS")?.extract()?;
    Ok(apps)
}

fn extract_settings(py: Python<'_>) -> PyResult<DjangoSettings> {
    let settings = py.import("django.conf")?.getattr("settings")?;

    let auth_user_model: String = settings
        .getattr("AUTH_USER_MODEL")
        .and_then(|v| v.extract())
        .unwrap_or_else(|_| "auth.User".into());

    let databases: Vec<String> = settings
        .getattr("DATABASES")
        .and_then(|d| d.call_method0("keys"))
        .and_then(|k| {
            let mut keys = Vec::new();
            for item in k.try_iter()? {
                let item = item?;
                keys.push(item.extract::<String>()?);
            }
            Ok(keys)
        })
        .unwrap_or_default();

    let middleware: Vec<String> = settings
        .getattr("MIDDLEWARE")
        .and_then(|v| v.extract())
        .unwrap_or_default();

    // Store framework-specific settings in the extra map
    let mut extra = std::collections::HashMap::new();
    if let Ok(drf) = settings.getattr("REST_FRAMEWORK") {
        if let Ok(auth) = drf
            .get_item("DEFAULT_AUTHENTICATION_CLASSES")
            .and_then(|v| v.str())
        {
            extra.insert(
                "drf.default_authentication_classes".into(),
                auth.to_string(),
            );
        }
        if let Ok(perm) = drf
            .get_item("DEFAULT_PERMISSION_CLASSES")
            .and_then(|v| v.str())
        {
            extra.insert("drf.default_permission_classes".into(), perm.to_string());
        }
        if let Ok(page) = drf
            .get_item("DEFAULT_PAGINATION_CLASS")
            .and_then(|v| v.extract::<String>())
        {
            extra.insert("drf.default_pagination_class".into(), page);
        }
    }

    Ok(DjangoSettings {
        auth_user_model,
        databases,
        middleware,
        extra,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn get_class_path(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    let cls = obj.getattr("__class__")?;
    let module: String = cls.getattr("__module__")?.extract()?;
    let name: String = cls.getattr("__name__")?.extract()?;
    Ok(format!("{module}.{name}"))
}

fn is_relational(field_class: &str) -> bool {
    field_class.contains("ForeignKey")
        || field_class.contains("OneToOneField")
        || field_class.contains("ManyToManyField")
        || field_class.contains("Rel")
}

fn field_class_to_python_type(field_class: &str) -> String {
    if field_class.contains("IntegerField") || field_class.contains("AutoField") {
        "int".into()
    } else if field_class.contains("FloatField") || field_class.contains("DecimalField") {
        "float".into()
    } else if field_class.contains("BooleanField") {
        "bool".into()
    } else if field_class.contains("DateTimeField") {
        "datetime".into()
    } else if field_class.contains("DateField") {
        "date".into()
    } else if field_class.contains("TimeField") {
        "time".into()
    } else if field_class.contains("UUIDField") {
        "UUID".into()
    } else if field_class.contains("JSONField") {
        "Any".into()
    } else if field_class.contains("BinaryField") {
        "bytes".into()
    } else {
        "str".into()
    }
}

fn extract_default(py: Python<'_>, field: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    let django_fields = py.import("django.db.models.fields")?;
    let not_provided = django_fields.getattr("NOT_PROVIDED")?;
    let default = field.getattr("default")?;

    if default.is(&not_provided) {
        return Ok(None);
    }

    if default.is_callable() {
        let name: String = default
            .getattr("__name__")
            .or_else(|_| default.getattr("__class__")?.getattr("__name__"))
            .and_then(|v| v.extract())
            .unwrap_or_else(|_| "unknown".into());
        return Ok(Some(format!("<callable: {name}>")));
    }

    Ok(Some(format!("{default}")))
}

fn extract_choices(field: &Bound<'_, PyAny>) -> PyResult<Vec<(String, String)>> {
    // Prefer flatchoices which is always a flat list of (value, label) pairs,
    // even for grouped choices and TextChoices/IntegerChoices enums.
    let choices_obj = field
        .getattr("flatchoices")
        .or_else(|_| field.getattr("choices"))?;

    if choices_obj.is_none() {
        // choices is Python None — no choices defined
        return Ok(vec![]);
    }

    // For EnumChoiceField the choices may live on an enum_class attribute.
    // If the resolved object is not iterable or is empty, fall back to that.
    let mut result = collect_flat_choices(&choices_obj);

    if result.is_empty() {
        // Try EnumChoiceField: field.enum_class.choices
        if let Ok(enum_cls) = field.getattr("enum_class") {
            if !enum_cls.is_none() {
                if let Ok(enum_choices) = enum_cls.getattr("choices") {
                    result = collect_flat_choices(&enum_choices);
                }
            }
        }
    }

    Ok(result)
}

/// Collect (value, label) pairs from a Django choices iterable.
/// Handles both flat choices `[(val, label), ...]` and grouped choices
/// `[(group, [(val, label), ...]), ...]`.
fn collect_flat_choices(choices_obj: &Bound<'_, PyAny>) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let Ok(iter) = choices_obj.try_iter() else {
        return result;
    };
    for item in iter.flatten() {
        let Ok(label_or_group) = item.get_item(1) else {
            continue;
        };
        // Grouped choices: item[1] is a list/tuple of (value, label) pairs
        if label_or_group.try_iter().is_ok()
            && !label_or_group.is_instance_of::<pyo3::types::PyString>()
        {
            // This is a group — recurse into sub-choices
            for sub in label_or_group.try_iter().into_iter().flatten().flatten() {
                let value = sub.get_item(0).map(|v| format!("{v}")).unwrap_or_default();
                let label = sub.get_item(1).map(|v| format!("{v}")).unwrap_or_default();
                if !value.is_empty() || !label.is_empty() {
                    result.push((value, label));
                }
            }
        } else {
            // Flat choice: item = (value, label)
            let value = item.get_item(0).map(|v| format!("{v}")).unwrap_or_default();
            let label = format!("{label_or_group}");
            if !value.is_empty() || !label.is_empty() {
                result.push((value, label));
            }
        }
    }
    result
}

fn extract_field_validators(field: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let validators = field.getattr("validators")?;
    let mut result = Vec::new();
    for v in validators.try_iter()? {
        let v = v?;
        result.push(get_class_path(&v)?);
    }
    Ok(result)
}

fn extract_public_methods(_py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let dir = obj.dir()?;
    let mut methods = Vec::new();
    for item in dir.iter() {
        let name_str: String = item.extract()?;
        // Skip private methods (_foo) but keep dunder methods (__str__, __repr__, etc.)
        if name_str.starts_with('_') && !name_str.starts_with("__") {
            continue;
        }
        if let Ok(attr) = obj.getattr(name_str.as_str()) {
            if attr.is_callable() {
                methods.push(name_str);
            }
        }
    }
    Ok(methods)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
