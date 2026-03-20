use super::cross::*;
use thorn_api::*;

// ── Test graph helpers ────────────────────────────────────────────────────

fn make_graph(models: Vec<Model>) -> AppGraph {
    AppGraph {
        models,
        installed_apps: vec![],
        settings: FrameworkSettings::default(),
    }
}

fn make_model(name: &str, fields: Vec<Field>, relations: Vec<Relation>) -> Model {
    Model {
        app_label: "test".into(),
        name: name.into(),
        db_table: name.to_lowercase().into(),
        module: "test.models".into(),
        source_file: "test/models.py".into(),
        abstract_model: false,
        proxy: false,
        fields,
        relations,
        managers: vec![Manager {
            name: "objects".into(),
            manager_class: "django.db.models.Manager".into(),
            queryset_class: "django.db.models.QuerySet".into(),
            is_default: true,
            custom_methods: vec![],
        }],
        parents: vec![],
        methods: vec!["__str__".into()],
    }
}

fn make_field(name: &str, field_class: &str) -> Field {
    Field {
        name: name.into(),
        column: name.into(),
        field_class: field_class.into(),
        native_type: "str".into(),
        nullable: false,
        blank: false,
        default: None,
        max_length: None,
        choices: vec![],
        validators: vec![],
        primary_key: false,
        unique: false,
        db_index: false,
    }
}

fn make_fk(name: &str, to_model: &str) -> Relation {
    Relation {
        name: name.into(),
        kind: RelationKind::ForeignKey,
        to_model: to_model.into(),
        to_model_app: "test".into(),
        related_name: format!("{}_set", name),
        related_query_name: name.into(),
        on_delete: Some("CASCADE".into()),
        nullable: false,
        through_model: None,
    }
}

fn run_check_with_graph(check: &dyn AstCheck, source: &str, graph: &AppGraph) -> Vec<String> {
    let parsed =
        ruff_python_parser::parse(source, ruff_python_parser::Mode::Module.into()).unwrap();
    let module = parsed.into_syntax().module().unwrap().clone();
    let ctx = CheckContext {
        module: &module,
        source,
        filename: "test.py",
        graph,
    };
    check.check(&ctx).into_iter().map(|d| d.code).collect()
}

// ── DJ201: InvalidFilterField ─────────────────────────────────────────────

#[test]
fn dj201_flags_nonexistent_filter_field() {
    let graph = make_graph(vec![make_model(
        "MyModel",
        vec![make_field("real_field", "CharField")],
        vec![],
    )]);
    let source = "MyModel.objects.filter(nonexistent_field=1)";
    let codes = run_check_with_graph(&InvalidFilterField, source, &graph);
    assert!(
        codes.contains(&"DJ201".to_string()),
        "expected DJ201 for nonexistent field in filter, got: {:?}",
        codes
    );
}

#[test]
fn dj201_no_error_for_existing_filter_field() {
    let graph = make_graph(vec![make_model(
        "MyModel",
        vec![make_field("real_field", "CharField")],
        vec![],
    )]);
    let source = "MyModel.objects.filter(real_field='hello')";
    let codes = run_check_with_graph(&InvalidFilterField, source, &graph);
    assert!(
        !codes.contains(&"DJ201".to_string()),
        "unexpected DJ201 for existing field in filter, got: {:?}",
        codes
    );
}

#[test]
fn dj201_allows_fk_id_suffix_in_filter() {
    // Filtering by `author_id` where `author` is a FK relation should be valid.
    let graph = make_graph(vec![make_model(
        "Post",
        vec![make_field("title", "CharField")],
        vec![make_fk("author", "User")],
    )]);
    let source = "Post.objects.filter(author_id=42)";
    let codes = run_check_with_graph(&InvalidFilterField, source, &graph);
    assert!(
        !codes.contains(&"DJ201".to_string()),
        "unexpected DJ201 for FK _id suffix in filter, got: {:?}",
        codes
    );
}

#[test]
fn dj201_flags_in_exclude_call() {
    let graph = make_graph(vec![make_model(
        "MyModel",
        vec![make_field("status", "CharField")],
        vec![],
    )]);
    let source = "MyModel.objects.exclude(ghost_field='x')";
    let codes = run_check_with_graph(&InvalidFilterField, source, &graph);
    assert!(
        codes.contains(&"DJ201".to_string()),
        "expected DJ201 for nonexistent field in exclude(), got: {:?}",
        codes
    );
}

#[test]
fn dj201_no_error_when_no_graph_models() {
    // Empty graph means we cannot validate; check should be silent.
    let graph = make_graph(vec![]);
    let source = "MyModel.objects.filter(anything=1)";
    let codes = run_check_with_graph(&InvalidFilterField, source, &graph);
    assert!(
        !codes.contains(&"DJ201".to_string()),
        "should not flag when graph has no models, got: {:?}",
        codes
    );
}

// ── DJ202: InvalidValuesField ─────────────────────────────────────────────

#[test]
fn dj202_flags_nonexistent_order_by_field() {
    let graph = make_graph(vec![make_model(
        "MyModel",
        vec![make_field("name", "CharField")],
        vec![],
    )]);
    let source = "MyModel.objects.order_by('nonexistent')";
    let codes = run_check_with_graph(&InvalidValuesField, source, &graph);
    assert!(
        codes.contains(&"DJ202".to_string()),
        "expected DJ202 for nonexistent field in order_by, got: {:?}",
        codes
    );
}

#[test]
fn dj202_allows_random_order_by() {
    // `order_by('?')` is Django's random ordering — always valid.
    let graph = make_graph(vec![make_model(
        "MyModel",
        vec![make_field("name", "CharField")],
        vec![],
    )]);
    let source = "MyModel.objects.order_by('?')";
    let codes = run_check_with_graph(&InvalidValuesField, source, &graph);
    assert!(
        !codes.contains(&"DJ202".to_string()),
        "unexpected DJ202 for order_by('?'), got: {:?}",
        codes
    );
}

#[test]
fn dj202_no_error_for_existing_order_by_field() {
    let graph = make_graph(vec![make_model(
        "MyModel",
        vec![make_field("name", "CharField")],
        vec![],
    )]);
    let source = "MyModel.objects.order_by('name')";
    let codes = run_check_with_graph(&InvalidValuesField, source, &graph);
    assert!(
        !codes.contains(&"DJ202".to_string()),
        "unexpected DJ202 for existing field in order_by, got: {:?}",
        codes
    );
}

#[test]
fn dj202_allows_descending_prefix_on_real_field() {
    // '-name' means order descending by 'name'; the '-' prefix should be stripped.
    let graph = make_graph(vec![make_model(
        "MyModel",
        vec![make_field("name", "CharField")],
        vec![],
    )]);
    let source = "MyModel.objects.order_by('-name')";
    let codes = run_check_with_graph(&InvalidValuesField, source, &graph);
    assert!(
        !codes.contains(&"DJ202".to_string()),
        "unexpected DJ202 for descending order '-name', got: {:?}",
        codes
    );
}

#[test]
fn dj202_flags_in_values_call() {
    let graph = make_graph(vec![make_model(
        "MyModel",
        vec![make_field("title", "CharField")],
        vec![],
    )]);
    let source = "MyModel.objects.values('missing_field')";
    let codes = run_check_with_graph(&InvalidValuesField, source, &graph);
    assert!(
        codes.contains(&"DJ202".to_string()),
        "expected DJ202 for nonexistent field in values(), got: {:?}",
        codes
    );
}

// ── DJ205: SerializerFieldMismatch ────────────────────────────────────────

#[test]
fn dj205_flags_nonexistent_serializer_field() {
    let graph = make_graph(vec![make_model(
        "Post",
        vec![make_field("title", "CharField")],
        vec![],
    )]);
    // Serializer references 'missing' which does not exist on Post.
    let source = r#"
class PostSerializer(ModelSerializer):
    class Meta:
        model = Post
        fields = ['title', 'missing']
"#;
    let codes = run_check_with_graph(&SerializerFieldMismatch, source, &graph);
    assert!(
        codes.contains(&"DJ205".to_string()),
        "expected DJ205 for nonexistent serializer field, got: {:?}",
        codes
    );
}

#[test]
fn dj205_no_error_for_existing_serializer_field() {
    let graph = make_graph(vec![make_model(
        "Post",
        vec![
            make_field("title", "CharField"),
            make_field("body", "TextField"),
        ],
        vec![],
    )]);
    let source = r#"
class PostSerializer(ModelSerializer):
    class Meta:
        model = Post
        fields = ['title', 'body']
"#;
    let codes = run_check_with_graph(&SerializerFieldMismatch, source, &graph);
    assert!(
        !codes.contains(&"DJ205".to_string()),
        "unexpected DJ205 for existing serializer fields, got: {:?}",
        codes
    );
}

#[test]
fn dj205_declared_serializer_method_field_does_not_trigger() {
    // A SerializerMethodField declared on the class body should suppress the check
    // because `computed` is declared as a class attribute.
    let graph = make_graph(vec![make_model(
        "Post",
        vec![make_field("title", "CharField")],
        vec![],
    )]);
    let source = r#"
class PostSerializer(ModelSerializer):
    computed = serializers.SerializerMethodField()

    class Meta:
        model = Post
        fields = ['title', 'computed']

    def get_computed(self, obj):
        return obj.title.upper()
"#;
    let codes = run_check_with_graph(&SerializerFieldMismatch, source, &graph);
    assert!(
        !codes.contains(&"DJ205".to_string()),
        "unexpected DJ205 for declared SerializerMethodField, got: {:?}",
        codes
    );
}

#[test]
fn dj205_all_fields_shorthand_never_triggers() {
    let graph = make_graph(vec![make_model(
        "Post",
        vec![make_field("title", "CharField")],
        vec![],
    )]);
    let source = r#"
class PostSerializer(ModelSerializer):
    class Meta:
        model = Post
        fields = '__all__'
"#;
    let codes = run_check_with_graph(&SerializerFieldMismatch, source, &graph);
    assert!(
        !codes.contains(&"DJ205".to_string()),
        "unexpected DJ205 for fields='__all__', got: {:?}",
        codes
    );
}

// ── DJ207: ForeignKeyIdAccess ─────────────────────────────────────────────

#[test]
fn dj207_flags_self_fk_dot_id_inside_model_class() {
    // `self.author.id` inside a model class that has a FK named `author` should trigger.
    let graph = make_graph(vec![make_model(
        "Post",
        vec![make_field("title", "CharField")],
        vec![make_fk("author", "User")],
    )]);
    let source = r#"
class Post(models.Model):
    def get_author_id(self):
        return self.author.id
"#;
    let codes = run_check_with_graph(&ForeignKeyIdAccess, source, &graph);
    assert!(
        codes.contains(&"DJ207".to_string()),
        "expected DJ207 for self.fk_field.id inside model class, got: {:?}",
        codes
    );
}

#[test]
fn dj207_no_error_outside_model_class() {
    // `self.author.id` outside any class known to have a FK named `author`
    // should not trigger — the check is scoped to the owning model class.
    let graph = make_graph(vec![make_model(
        "Post",
        vec![make_field("title", "CharField")],
        vec![make_fk("author", "User")],
    )]);
    // This is a view/service class, not the Post model itself.
    let source = r#"
class PostView:
    def render(self):
        return self.author.id
"#;
    let codes = run_check_with_graph(&ForeignKeyIdAccess, source, &graph);
    assert!(
        !codes.contains(&"DJ207".to_string()),
        "unexpected DJ207 outside model class, got: {:?}",
        codes
    );
}

#[test]
fn dj207_no_error_for_non_fk_attribute_access() {
    // `self.title.id` where `title` is a plain CharField (not a FK) should not trigger.
    let graph = make_graph(vec![make_model(
        "Post",
        vec![make_field("title", "CharField")],
        vec![],
    )]);
    let source = r#"
class Post(models.Model):
    def get_title_id(self):
        return self.title.id
"#;
    let codes = run_check_with_graph(&ForeignKeyIdAccess, source, &graph);
    assert!(
        !codes.contains(&"DJ207".to_string()),
        "unexpected DJ207 for non-FK attribute access, got: {:?}",
        codes
    );
}

#[test]
fn dj207_no_error_when_no_graph_models() {
    let graph = make_graph(vec![]);
    let source = r#"
class Post(models.Model):
    def get_author_id(self):
        return self.author.id
"#;
    let codes = run_check_with_graph(&ForeignKeyIdAccess, source, &graph);
    assert!(
        !codes.contains(&"DJ207".to_string()),
        "should not flag when graph has no models, got: {:?}",
        codes
    );
}
