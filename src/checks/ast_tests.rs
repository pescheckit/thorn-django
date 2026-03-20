//! Tests for AST checks DJ027-DJ050 and pylint-django compatibility checks.

use super::ast::*;
use thorn_api::{AppGraph, AstCheck, CheckContext};

fn run_check(check: &dyn AstCheck, source: &str) -> Vec<String> {
    let parsed =
        ruff_python_parser::parse(source, ruff_python_parser::Mode::Module.into()).unwrap();
    let module = parsed.into_syntax().module().unwrap().clone();
    let graph = AppGraph::default();
    let ctx = CheckContext {
        module: &module,
        source,
        filename: "test.py",
        graph: &graph,
    };
    check.check(&ctx).into_iter().map(|d| d.code).collect()
}

/// Run a check using a specific filename (needed for checks that gate on path patterns).
fn run_check_with_filename(check: &dyn AstCheck, source: &str, filename: &str) -> Vec<String> {
    let parsed =
        ruff_python_parser::parse(source, ruff_python_parser::Mode::Module.into()).unwrap();
    let module = parsed.into_syntax().module().unwrap().clone();
    let graph = AppGraph::default();
    let ctx = CheckContext {
        module: &module,
        source,
        filename,
        graph: &graph,
    };
    check.check(&ctx).into_iter().map(|d| d.code).collect()
}

// ── DJ027: CeleryDelayInAtomic ────────────────────────────────────────────

#[test]
fn dj027_triggers_delay_inside_atomic() {
    let src = r#"
with transaction.atomic():
    my_task.delay()
"#;
    let codes = run_check(&CeleryDelayInAtomic, src);
    assert!(
        codes.contains(&"DJ027".to_string()),
        "expected DJ027, got {:?}",
        codes
    );
}

#[test]
fn dj027_triggers_apply_async_inside_atomic() {
    let src = r#"
with transaction.atomic():
    my_task.apply_async(args=[1])
"#;
    let codes = run_check(&CeleryDelayInAtomic, src);
    assert!(
        codes.contains(&"DJ027".to_string()),
        "expected DJ027, got {:?}",
        codes
    );
}

#[test]
fn dj027_no_trigger_delay_outside_atomic() {
    let src = r#"
my_task.delay()
"#;
    let codes = run_check(&CeleryDelayInAtomic, src);
    assert!(
        !codes.contains(&"DJ027".to_string()),
        "unexpected DJ027, got {:?}",
        codes
    );
}

#[test]
fn dj027_no_trigger_unrelated_with_block() {
    let src = r#"
with open("file.txt") as f:
    my_task.delay()
"#;
    let codes = run_check(&CeleryDelayInAtomic, src);
    assert!(
        !codes.contains(&"DJ027".to_string()),
        "unexpected DJ027, got {:?}",
        codes
    );
}

// ── DJ028: RedirectReverse ────────────────────────────────────────────────

#[test]
fn dj028_triggers_redirect_reverse() {
    let src = r#"
response = redirect(reverse("home"))
"#;
    let codes = run_check(&RedirectReverse, src);
    assert!(
        codes.contains(&"DJ028".to_string()),
        "expected DJ028, got {:?}",
        codes
    );
}

#[test]
fn dj028_no_trigger_redirect_string() {
    let src = r#"
response = redirect("home")
"#;
    let codes = run_check(&RedirectReverse, src);
    assert!(
        !codes.contains(&"DJ028".to_string()),
        "unexpected DJ028, got {:?}",
        codes
    );
}

#[test]
fn dj028_no_trigger_reverse_alone() {
    let src = r#"
url = reverse("home")
"#;
    let codes = run_check(&RedirectReverse, src);
    assert!(
        !codes.contains(&"DJ028".to_string()),
        "unexpected DJ028, got {:?}",
        codes
    );
}

// ── DJ029: UnfilteredDelete ───────────────────────────────────────────────

#[test]
fn dj029_triggers_objects_all_delete() {
    let src = r#"
MyModel.objects.all().delete()
"#;
    let codes = run_check(&UnfilteredDelete, src);
    assert!(
        codes.contains(&"DJ029".to_string()),
        "expected DJ029, got {:?}",
        codes
    );
}

#[test]
fn dj029_triggers_objects_delete_directly() {
    let src = r#"
MyModel.objects.delete()
"#;
    let codes = run_check(&UnfilteredDelete, src);
    assert!(
        codes.contains(&"DJ029".to_string()),
        "expected DJ029, got {:?}",
        codes
    );
}

#[test]
fn dj029_no_trigger_filtered_delete() {
    let src = r#"
MyModel.objects.filter(active=False).delete()
"#;
    let codes = run_check(&UnfilteredDelete, src);
    assert!(
        !codes.contains(&"DJ029".to_string()),
        "unexpected DJ029, got {:?}",
        codes
    );
}

#[test]
fn dj029_no_trigger_in_tests_file() {
    let src = r#"
MyModel.objects.all().delete()
"#;
    let codes = run_check_with_filename(&UnfilteredDelete, src, "/app/tests/test_model.py");
    assert!(
        !codes.contains(&"DJ029".to_string()),
        "unexpected DJ029 in tests file, got {:?}",
        codes
    );
}

// ── DJ030: DRFAllowAnyPermission ──────────────────────────────────────────

#[test]
fn dj030_triggers_allow_any_in_view() {
    let src = r#"
class MyView(APIView):
    permission_classes = [AllowAny]
"#;
    let codes = run_check(&DRFAllowAnyPermission, src);
    assert!(
        codes.contains(&"DJ030".to_string()),
        "expected DJ030, got {:?}",
        codes
    );
}

#[test]
fn dj030_triggers_empty_permission_classes() {
    let src = r#"
class MyView(APIView):
    permission_classes = []
"#;
    let codes = run_check(&DRFAllowAnyPermission, src);
    assert!(
        codes.contains(&"DJ030".to_string()),
        "expected DJ030, got {:?}",
        codes
    );
}

#[test]
fn dj030_no_trigger_authenticated_permission() {
    let src = r#"
class MyView(APIView):
    permission_classes = [IsAuthenticated]
"#;
    let codes = run_check(&DRFAllowAnyPermission, src);
    assert!(
        !codes.contains(&"DJ030".to_string()),
        "unexpected DJ030, got {:?}",
        codes
    );
}

#[test]
fn dj030_no_trigger_non_drf_class() {
    let src = r#"
class MyPlainClass:
    permission_classes = [AllowAny]
"#;
    let codes = run_check(&DRFAllowAnyPermission, src);
    assert!(
        !codes.contains(&"DJ030".to_string()),
        "unexpected DJ030 on non-DRF class, got {:?}",
        codes
    );
}

// ── DJ031: DRFEmptyAuthClasses ────────────────────────────────────────────

#[test]
fn dj031_triggers_empty_auth_classes() {
    let src = r#"
class MyView(APIView):
    authentication_classes = []
"#;
    let codes = run_check(&DRFEmptyAuthClasses, src);
    assert!(
        codes.contains(&"DJ031".to_string()),
        "expected DJ031, got {:?}",
        codes
    );
}

#[test]
fn dj031_no_trigger_with_auth_classes() {
    let src = r#"
class MyView(APIView):
    authentication_classes = [SessionAuthentication]
"#;
    let codes = run_check(&DRFEmptyAuthClasses, src);
    assert!(
        !codes.contains(&"DJ031".to_string()),
        "unexpected DJ031, got {:?}",
        codes
    );
}

#[test]
fn dj031_no_trigger_non_drf_class() {
    let src = r#"
class Config:
    authentication_classes = []
"#;
    let codes = run_check(&DRFEmptyAuthClasses, src);
    assert!(
        !codes.contains(&"DJ031".to_string()),
        "unexpected DJ031 on non-DRF class, got {:?}",
        codes
    );
}

// ── DJ032: DjangoValidationErrorInDRF ─────────────────────────────────────

#[test]
fn dj032_triggers_raise_django_ve_in_drf_code() {
    let src = r#"
from django.core.exceptions import ValidationError
from rest_framework import serializers

def validate_value(value):
    raise ValidationError("bad value")
"#;
    let codes = run_check(&DjangoValidationErrorInDRF, src);
    assert!(
        codes.contains(&"DJ032".to_string()),
        "expected DJ032, got {:?}",
        codes
    );
}

#[test]
fn dj032_no_trigger_except_django_ve_not_raise() {
    // except ValidationError is catching it, not raising — must NOT trigger
    let src = r#"
from django.core.exceptions import ValidationError
from rest_framework import serializers

def my_func():
    try:
        do_something()
    except ValidationError:
        pass
"#;
    let codes = run_check(&DjangoValidationErrorInDRF, src);
    assert!(
        !codes.contains(&"DJ032".to_string()),
        "unexpected DJ032 for except handler, got {:?}",
        codes
    );
}

#[test]
fn dj032_no_trigger_without_rest_framework_import() {
    // no rest_framework import → check should not fire
    let src = r#"
from django.core.exceptions import ValidationError

def validate_value(value):
    raise ValidationError("bad value")
"#;
    let codes = run_check(&DjangoValidationErrorInDRF, src);
    assert!(
        !codes.contains(&"DJ032".to_string()),
        "unexpected DJ032 without DRF import, got {:?}",
        codes
    );
}

#[test]
fn dj032_no_trigger_without_django_ve_import() {
    // no django ValidationError import → check should not fire
    let src = r#"
from rest_framework.exceptions import ValidationError
from rest_framework import serializers

def validate_value(value):
    raise ValidationError("bad value")
"#;
    let codes = run_check(&DjangoValidationErrorInDRF, src);
    assert!(
        !codes.contains(&"DJ032".to_string()),
        "unexpected DJ032 with DRF VE, got {:?}",
        codes
    );
}

// ── DJ033: DRFNoPaginationClass ───────────────────────────────────────────

#[test]
fn dj033_triggers_list_api_view_no_pagination() {
    let src = r#"
class MyListView(ListAPIView):
    queryset = MyModel.objects.all()
    serializer_class = MySerializer
"#;
    let codes = run_check(&DRFNoPaginationClass, src);
    assert!(
        codes.contains(&"DJ033".to_string()),
        "expected DJ033, got {:?}",
        codes
    );
}

#[test]
fn dj033_triggers_model_viewset_no_pagination() {
    let src = r#"
class MyViewSet(ModelViewSet):
    queryset = MyModel.objects.all()
    serializer_class = MySerializer
"#;
    let codes = run_check(&DRFNoPaginationClass, src);
    assert!(
        codes.contains(&"DJ033".to_string()),
        "expected DJ033, got {:?}",
        codes
    );
}

#[test]
fn dj033_no_trigger_with_pagination_class() {
    let src = r#"
class MyListView(ListAPIView):
    queryset = MyModel.objects.all()
    serializer_class = MySerializer
    pagination_class = PageNumberPagination
"#;
    let codes = run_check(&DRFNoPaginationClass, src);
    assert!(
        !codes.contains(&"DJ033".to_string()),
        "unexpected DJ033, got {:?}",
        codes
    );
}

#[test]
fn dj033_no_trigger_without_queryset_binding() {
    // A base/mixin class that has no queryset should be skipped
    let src = r#"
class MyBaseMixin(ListAPIView):
    serializer_class = MySerializer
"#;
    let codes = run_check(&DRFNoPaginationClass, src);
    assert!(
        !codes.contains(&"DJ033".to_string()),
        "unexpected DJ033 on mixin, got {:?}",
        codes
    );
}

// ── DJ034: TooManyArguments ───────────────────────────────────────────────

#[test]
fn dj034_triggers_too_many_arguments() {
    // Default max is 5; we supply 6 non-self args
    let src = r#"
def my_func(a, b, c, d, e, f):
    pass
"#;
    let check = TooManyArguments { max_args: 5 };
    let codes = run_check(&check, src);
    assert!(
        codes.contains(&"DJ034".to_string()),
        "expected DJ034, got {:?}",
        codes
    );
}

#[test]
fn dj034_no_trigger_within_limit() {
    let src = r#"
def my_func(a, b, c):
    pass
"#;
    let check = TooManyArguments { max_args: 5 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ034".to_string()),
        "unexpected DJ034, got {:?}",
        codes
    );
}

#[test]
fn dj034_no_trigger_for_init() {
    // __init__ is explicitly exempt from this check
    let src = r#"
class Foo:
    def __init__(self, a, b, c, d, e, f):
        pass
"#;
    let check = TooManyArguments { max_args: 5 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ034".to_string()),
        "unexpected DJ034 for __init__, got {:?}",
        codes
    );
}

#[test]
fn dj034_self_and_cls_excluded_from_count() {
    // self + 5 args = 5 real args, should NOT trigger at max=5
    let src = r#"
class Foo:
    def my_method(self, a, b, c, d, e):
        pass
"#;
    let check = TooManyArguments { max_args: 5 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ034".to_string()),
        "unexpected DJ034 when self excluded, got {:?}",
        codes
    );
}

// ── DJ035: TooManyReturnStatements ────────────────────────────────────────

#[test]
fn dj035_triggers_too_many_returns() {
    let src = r#"
def classify(x):
    if x == 1:
        return "one"
    if x == 2:
        return "two"
    if x == 3:
        return "three"
    if x == 4:
        return "four"
    return "other"
"#;
    let check = TooManyReturnStatements { max_returns: 3 };
    let codes = run_check(&check, src);
    assert!(
        codes.contains(&"DJ035".to_string()),
        "expected DJ035, got {:?}",
        codes
    );
}

#[test]
fn dj035_no_trigger_within_limit() {
    let src = r#"
def decide(x):
    if x > 0:
        return "positive"
    return "non-positive"
"#;
    let check = TooManyReturnStatements { max_returns: 3 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ035".to_string()),
        "unexpected DJ035, got {:?}",
        codes
    );
}

// ── DJ036: TooManyBranches ────────────────────────────────────────────────

#[test]
fn dj036_triggers_too_many_branches() {
    let src = r#"
def complex_func(x):
    if x == 1:
        pass
    elif x == 2:
        pass
    elif x == 3:
        pass
    elif x == 4:
        pass
    elif x == 5:
        pass
    else:
        pass
"#;
    // The if counts as 1, each elif/else also counts; total = 1 + 5 = 6
    let check = TooManyBranches { max_branches: 4 };
    let codes = run_check(&check, src);
    assert!(
        codes.contains(&"DJ036".to_string()),
        "expected DJ036, got {:?}",
        codes
    );
}

#[test]
fn dj036_no_trigger_within_limit() {
    let src = r#"
def simple(x):
    if x > 0:
        return True
    return False
"#;
    let check = TooManyBranches { max_branches: 4 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ036".to_string()),
        "unexpected DJ036, got {:?}",
        codes
    );
}

#[test]
fn dj036_no_trigger_in_test_file() {
    let src = r#"
def test_complex():
    if True:
        pass
    elif True:
        pass
    elif True:
        pass
    elif True:
        pass
    elif True:
        pass
"#;
    let check = TooManyBranches { max_branches: 2 };
    let codes = run_check_with_filename(&check, src, "/app/tests/test_views.py");
    assert!(
        !codes.contains(&"DJ036".to_string()),
        "unexpected DJ036 in test file, got {:?}",
        codes
    );
}

// ── DJ037: TooManyLocalVariables ──────────────────────────────────────────

#[test]
fn dj037_triggers_too_many_locals() {
    let src = r#"
def big_func():
    a = 1
    b = 2
    c = 3
    d = 4
    e = 5
    f = 6
"#;
    let check = TooManyLocalVariables { max_locals: 4 };
    let codes = run_check(&check, src);
    assert!(
        codes.contains(&"DJ037".to_string()),
        "expected DJ037, got {:?}",
        codes
    );
}

#[test]
fn dj037_no_trigger_within_limit() {
    let src = r#"
def small_func():
    x = 1
    y = 2
"#;
    let check = TooManyLocalVariables { max_locals: 4 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ037".to_string()),
        "unexpected DJ037, got {:?}",
        codes
    );
}

// ── DJ038: TooManyStatements ──────────────────────────────────────────────

#[test]
fn dj038_triggers_too_many_statements() {
    let src = r#"
def verbose():
    x = 1
    y = 2
    z = 3
    a = 4
    b = 5
    c = 6
"#;
    let check = TooManyStatements { max_statements: 4 };
    let codes = run_check(&check, src);
    assert!(
        codes.contains(&"DJ038".to_string()),
        "expected DJ038, got {:?}",
        codes
    );
}

#[test]
fn dj038_no_trigger_within_limit() {
    let src = r#"
def tiny():
    x = 1
    y = 2
"#;
    let check = TooManyStatements { max_statements: 4 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ038".to_string()),
        "unexpected DJ038, got {:?}",
        codes
    );
}

// ── DJ039: ModelTooManyFields ─────────────────────────────────────────────

#[test]
fn dj039_triggers_model_too_many_fields() {
    let src = r#"
class BigModel(Model):
    a = CharField()
    b = IntegerField()
    c = TextField()
    d = BooleanField()
    e = DateField()
    f = EmailField()
"#;
    let check = ModelTooManyFields { max_fields: 4 };
    let codes = run_check(&check, src);
    assert!(
        codes.contains(&"DJ039".to_string()),
        "expected DJ039, got {:?}",
        codes
    );
}

#[test]
fn dj039_no_trigger_within_limit() {
    let src = r#"
class SmallModel(Model):
    name = CharField()
    age = IntegerField()
"#;
    let check = ModelTooManyFields { max_fields: 4 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ039".to_string()),
        "unexpected DJ039, got {:?}",
        codes
    );
}

#[test]
fn dj039_no_trigger_non_model_class() {
    let src = r#"
class NotAModel:
    a = CharField()
    b = IntegerField()
    c = TextField()
    d = BooleanField()
    e = DateField()
"#;
    let check = ModelTooManyFields { max_fields: 3 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ039".to_string()),
        "unexpected DJ039 on non-Model class, got {:?}",
        codes
    );
}

// ── DJ040: TooManyMethods ─────────────────────────────────────────────────

#[test]
fn dj040_triggers_view_too_many_methods() {
    let src = r#"
class MyView(APIView):
    def get(self): pass
    def post(self): pass
    def put(self): pass
    def patch(self): pass
    def delete(self): pass
"#;
    let check = TooManyMethods { max_methods: 3 };
    let codes = run_check(&check, src);
    assert!(
        codes.contains(&"DJ040".to_string()),
        "expected DJ040, got {:?}",
        codes
    );
}

#[test]
fn dj040_no_trigger_within_limit() {
    let src = r#"
class MyView(APIView):
    def get(self): pass
    def post(self): pass
"#;
    let check = TooManyMethods { max_methods: 3 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ040".to_string()),
        "unexpected DJ040, got {:?}",
        codes
    );
}

#[test]
fn dj040_no_trigger_non_drf_class() {
    // Only DRF views and serializers are checked
    let src = r#"
class PlainHelper:
    def method_a(self): pass
    def method_b(self): pass
    def method_c(self): pass
    def method_d(self): pass
"#;
    let check = TooManyMethods { max_methods: 2 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ040".to_string()),
        "unexpected DJ040 on plain class, got {:?}",
        codes
    );
}

// ── DJ041: DeeplyNestedCode ───────────────────────────────────────────────

#[test]
fn dj041_triggers_deeply_nested_code() {
    let src = r#"
def process(x):
    if x:
        for item in x:
            if item:
                while True:
                    pass
"#;
    let check = DeeplyNestedCode { max_depth: 2 };
    let codes = run_check(&check, src);
    assert!(
        codes.contains(&"DJ041".to_string()),
        "expected DJ041, got {:?}",
        codes
    );
}

#[test]
fn dj041_no_trigger_within_depth() {
    let src = r#"
def process(x):
    if x:
        return x
"#;
    let check = DeeplyNestedCode { max_depth: 2 };
    let codes = run_check(&check, src);
    assert!(
        !codes.contains(&"DJ041".to_string()),
        "unexpected DJ041, got {:?}",
        codes
    );
}

#[test]
fn dj041_no_trigger_in_test_file() {
    let src = r#"
def test_nested():
    if True:
        for i in range(10):
            if i:
                while True:
                    pass
"#;
    let check = DeeplyNestedCode { max_depth: 1 };
    let codes = run_check_with_filename(&check, src, "/app/tests/test_logic.py");
    assert!(
        !codes.contains(&"DJ041".to_string()),
        "unexpected DJ041 in test file, got {:?}",
        codes
    );
}

// ── DJ044: SuperInitNotCalled ─────────────────────────────────────────────

#[test]
fn dj044_triggers_init_without_super_call() {
    let src = r#"
class MyForm(BaseForm):
    def __init__(self, *args, **kwargs):
        self.extra = True
"#;
    let codes = run_check(&SuperInitNotCalled, src);
    assert!(
        codes.contains(&"DJ044".to_string()),
        "expected DJ044, got {:?}",
        codes
    );
}

#[test]
fn dj044_no_trigger_when_super_init_called() {
    let src = r#"
class MyForm(BaseForm):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.extra = True
"#;
    let codes = run_check(&SuperInitNotCalled, src);
    assert!(
        !codes.contains(&"DJ044".to_string()),
        "unexpected DJ044, got {:?}",
        codes
    );
}

#[test]
fn dj044_no_trigger_class_with_no_bases() {
    // A class with no meaningful bases should not trigger
    let src = r#"
class Standalone:
    def __init__(self):
        self.x = 1
"#;
    let codes = run_check(&SuperInitNotCalled, src);
    assert!(
        !codes.contains(&"DJ044".to_string()),
        "unexpected DJ044 for class without bases, got {:?}",
        codes
    );
}

#[test]
fn dj044_no_trigger_for_object_base() {
    // class Foo(object) — object is not a meaningful base
    let src = r#"
class Foo(object):
    def __init__(self):
        self.x = 1
"#;
    let codes = run_check(&SuperInitNotCalled, src);
    assert!(
        !codes.contains(&"DJ044".to_string()),
        "unexpected DJ044 for class(object), got {:?}",
        codes
    );
}

#[test]
fn dj044_no_trigger_with_parent_class_init_style() {
    // ParentClass.__init__(self, ...) style also counts as calling super init
    let src = r#"
class MyWidget(BaseWidget):
    def __init__(self, *args, **kwargs):
        BaseWidget.__init__(self, *args, **kwargs)
"#;
    let codes = run_check(&SuperInitNotCalled, src);
    assert!(
        !codes.contains(&"DJ044".to_string()),
        "unexpected DJ044 with explicit parent init, got {:?}",
        codes
    );
}

// ── DJ045: BadExceptOrder ─────────────────────────────────────────────────

#[test]
fn dj045_triggers_broader_before_narrower() {
    let src = r#"
try:
    risky()
except Exception:
    pass
except ValueError:
    pass
"#;
    let codes = run_check(&BadExceptOrder, src);
    assert!(
        codes.contains(&"DJ045".to_string()),
        "expected DJ045, got {:?}",
        codes
    );
}

#[test]
fn dj045_triggers_lookup_error_before_key_error() {
    let src = r#"
try:
    risky()
except LookupError:
    pass
except KeyError:
    pass
"#;
    let codes = run_check(&BadExceptOrder, src);
    assert!(
        codes.contains(&"DJ045".to_string()),
        "expected DJ045, got {:?}",
        codes
    );
}

#[test]
fn dj045_no_trigger_correct_order_narrower_first() {
    let src = r#"
try:
    risky()
except ValueError:
    pass
except Exception:
    pass
"#;
    let codes = run_check(&BadExceptOrder, src);
    assert!(
        !codes.contains(&"DJ045".to_string()),
        "unexpected DJ045 for correct order, got {:?}",
        codes
    );
}

#[test]
fn dj045_no_trigger_unrelated_exceptions() {
    let src = r#"
try:
    risky()
except ValueError:
    pass
except KeyError:
    pass
"#;
    let codes = run_check(&BadExceptOrder, src);
    assert!(
        !codes.contains(&"DJ045".to_string()),
        "unexpected DJ045 for unrelated exceptions, got {:?}",
        codes
    );
}

// ── DJ046: UsingConstantTest ──────────────────────────────────────────────

#[test]
fn dj046_triggers_if_true() {
    let src = r#"
if True:
    do_something()
"#;
    let codes = run_check(&UsingConstantTest, src);
    assert!(
        codes.contains(&"DJ046".to_string()),
        "expected DJ046, got {:?}",
        codes
    );
}

#[test]
fn dj046_triggers_if_false() {
    let src = r#"
if False:
    do_something()
"#;
    let codes = run_check(&UsingConstantTest, src);
    assert!(
        codes.contains(&"DJ046".to_string()),
        "expected DJ046, got {:?}",
        codes
    );
}

#[test]
fn dj046_no_trigger_if_variable() {
    let src = r#"
if condition:
    do_something()
"#;
    let codes = run_check(&UsingConstantTest, src);
    assert!(
        !codes.contains(&"DJ046".to_string()),
        "unexpected DJ046, got {:?}",
        codes
    );
}

#[test]
fn dj046_no_trigger_if_type_checking() {
    // `if TYPE_CHECKING:` is a recognised pattern and must not be flagged
    let src = r#"
if TYPE_CHECKING:
    from mymodule import MyType
"#;
    let codes = run_check(&UsingConstantTest, src);
    assert!(
        !codes.contains(&"DJ046".to_string()),
        "unexpected DJ046 for TYPE_CHECKING, got {:?}",
        codes
    );
}

#[test]
fn dj046_no_trigger_while_true() {
    // `while True:` is an idiomatic infinite loop and must not be flagged
    let src = r#"
while True:
    process_queue()
"#;
    let codes = run_check(&UsingConstantTest, src);
    assert!(
        !codes.contains(&"DJ046".to_string()),
        "unexpected DJ046 for while True, got {:?}",
        codes
    );
}

// ── DJ048: SelfAssigningVariable ──────────────────────────────────────────

#[test]
fn dj048_triggers_self_assignment() {
    let src = r#"
x = x
"#;
    let codes = run_check(&SelfAssigningVariable, src);
    assert!(
        codes.contains(&"DJ048".to_string()),
        "expected DJ048, got {:?}",
        codes
    );
}

#[test]
fn dj048_triggers_attribute_self_assignment() {
    let src = r#"
self.name = self.name
"#;
    let codes = run_check(&SelfAssigningVariable, src);
    assert!(
        codes.contains(&"DJ048".to_string()),
        "expected DJ048, got {:?}",
        codes
    );
}

#[test]
fn dj048_no_trigger_normal_assignment() {
    let src = r#"
x = y
"#;
    let codes = run_check(&SelfAssigningVariable, src);
    assert!(
        !codes.contains(&"DJ048".to_string()),
        "unexpected DJ048, got {:?}",
        codes
    );
}

#[test]
fn dj048_no_trigger_attribute_to_different_target() {
    let src = r#"
self.name = other.name
"#;
    let codes = run_check(&SelfAssigningVariable, src);
    assert!(
        !codes.contains(&"DJ048".to_string()),
        "unexpected DJ048, got {:?}",
        codes
    );
}

// ── E5101: ModelUnicodeNotCallable ────────────────────────────────────────

#[test]
fn e5101_triggers_unicode_assigned_string_literal() {
    let src = r#"
class MyModel(Model):
    __unicode__ = "not callable"
"#;
    let codes = run_check(&ModelUnicodeNotCallable, src);
    assert!(
        codes.contains(&"E5101".to_string()),
        "expected E5101, got {:?}",
        codes
    );
}

#[test]
fn e5101_triggers_unicode_assigned_number() {
    let src = r#"
class MyModel(Model):
    __unicode__ = 42
"#;
    let codes = run_check(&ModelUnicodeNotCallable, src);
    assert!(
        codes.contains(&"E5101".to_string()),
        "expected E5101, got {:?}",
        codes
    );
}

#[test]
fn e5101_no_trigger_unicode_assigned_lambda() {
    let src = r#"
class MyModel(Model):
    __unicode__ = lambda self: self.name
"#;
    let codes = run_check(&ModelUnicodeNotCallable, src);
    assert!(
        !codes.contains(&"E5101".to_string()),
        "unexpected E5101 for lambda, got {:?}",
        codes
    );
}

#[test]
fn e5101_no_trigger_outside_model() {
    let src = r#"
class NotAModel:
    __unicode__ = "literal"
"#;
    let codes = run_check(&ModelUnicodeNotCallable, src);
    assert!(
        !codes.contains(&"E5101".to_string()),
        "unexpected E5101 on non-Model class, got {:?}",
        codes
    );
}

// ── W5102: ModelHasUnicode ────────────────────────────────────────────────

#[test]
fn w5102_triggers_model_defines_unicode_method() {
    let src = r#"
class MyModel(Model):
    def __unicode__(self):
        return self.name
"#;
    let codes = run_check(&ModelHasUnicode, src);
    assert!(
        codes.contains(&"W5102".to_string()),
        "expected W5102, got {:?}",
        codes
    );
}

#[test]
fn w5102_no_trigger_model_defines_str_method() {
    let src = r#"
class MyModel(Model):
    def __str__(self):
        return self.name
"#;
    let codes = run_check(&ModelHasUnicode, src);
    assert!(
        !codes.contains(&"W5102".to_string()),
        "unexpected W5102, got {:?}",
        codes
    );
}

#[test]
fn w5102_no_trigger_unicode_on_non_model() {
    let src = r#"
class HelperClass:
    def __unicode__(self):
        return "helper"
"#;
    let codes = run_check(&ModelHasUnicode, src);
    assert!(
        !codes.contains(&"W5102".to_string()),
        "unexpected W5102 on non-Model class, got {:?}",
        codes
    );
}

// ── E5141: HardCodedAuthUser ──────────────────────────────────────────────

#[test]
fn e5141_triggers_auth_user_string() {
    let src = r#"
owner = ForeignKey("auth.User", on_delete=CASCADE)
"#;
    let codes = run_check(&HardCodedAuthUser, src);
    assert!(
        codes.contains(&"E5141".to_string()),
        "expected E5141, got {:?}",
        codes
    );
}

#[test]
fn e5141_no_trigger_settings_auth_user_model() {
    let src = r#"
owner = ForeignKey(settings.AUTH_USER_MODEL, on_delete=CASCADE)
"#;
    let codes = run_check(&HardCodedAuthUser, src);
    assert!(
        !codes.contains(&"E5141".to_string()),
        "unexpected E5141, got {:?}",
        codes
    );
}

#[test]
fn e5141_no_trigger_other_string() {
    let src = r#"
name = "auth.Permission"
"#;
    let codes = run_check(&HardCodedAuthUser, src);
    assert!(
        !codes.contains(&"E5141".to_string()),
        "unexpected E5141 for other string, got {:?}",
        codes
    );
}

// ── E5142: ImportedAuthUser ───────────────────────────────────────────────

#[test]
fn e5142_triggers_import_user_from_auth_models() {
    let src = r#"
from django.contrib.auth.models import User
"#;
    let codes = run_check(&ImportedAuthUser, src);
    assert!(
        codes.contains(&"E5142".to_string()),
        "expected E5142, got {:?}",
        codes
    );
}

#[test]
fn e5142_no_trigger_get_user_model() {
    let src = r#"
from django.contrib.auth import get_user_model
User = get_user_model()
"#;
    let codes = run_check(&ImportedAuthUser, src);
    assert!(
        !codes.contains(&"E5142".to_string()),
        "unexpected E5142, got {:?}",
        codes
    );
}

#[test]
fn e5142_no_trigger_import_other_from_auth_models() {
    let src = r#"
from django.contrib.auth.models import Permission
"#;
    let codes = run_check(&ImportedAuthUser, src);
    assert!(
        !codes.contains(&"E5142".to_string()),
        "unexpected E5142 for Permission import, got {:?}",
        codes
    );
}

// ── R5101: HttpResponseWithJsonDumps ──────────────────────────────────────

#[test]
fn r5101_triggers_http_response_json_dumps() {
    let src = r#"
return HttpResponse(json.dumps(data))
"#;
    let codes = run_check(&HttpResponseWithJsonDumps, src);
    assert!(
        codes.contains(&"R5101".to_string()),
        "expected R5101, got {:?}",
        codes
    );
}

#[test]
fn r5101_no_trigger_json_response() {
    let src = r#"
return JsonResponse(data)
"#;
    let codes = run_check(&HttpResponseWithJsonDumps, src);
    assert!(
        !codes.contains(&"R5101".to_string()),
        "unexpected R5101, got {:?}",
        codes
    );
}

#[test]
fn r5101_no_trigger_http_response_plain_string() {
    let src = r#"
return HttpResponse("hello world")
"#;
    let codes = run_check(&HttpResponseWithJsonDumps, src);
    assert!(
        !codes.contains(&"R5101".to_string()),
        "unexpected R5101, got {:?}",
        codes
    );
}

// ── R5102: HttpResponseWithContentTypeJson ────────────────────────────────

#[test]
fn r5102_triggers_http_response_with_json_content_type() {
    let src = r#"
return HttpResponse(data, content_type="application/json")
"#;
    let codes = run_check(&HttpResponseWithContentTypeJson, src);
    assert!(
        codes.contains(&"R5102".to_string()),
        "expected R5102, got {:?}",
        codes
    );
}

#[test]
fn r5102_no_trigger_http_response_html_content_type() {
    let src = r#"
return HttpResponse(data, content_type="text/html")
"#;
    let codes = run_check(&HttpResponseWithContentTypeJson, src);
    assert!(
        !codes.contains(&"R5102".to_string()),
        "unexpected R5102, got {:?}",
        codes
    );
}

#[test]
fn r5102_no_trigger_json_response_with_content_type() {
    // JsonResponse already sets the content type — only HttpResponse is flagged here
    let src = r#"
return JsonResponse(data, content_type="application/json")
"#;
    let codes = run_check(&HttpResponseWithContentTypeJson, src);
    assert!(
        !codes.contains(&"R5102".to_string()),
        "unexpected R5102 for JsonResponse, got {:?}",
        codes
    );
}

// ── R5103: RedundantContentTypeForJsonResponse ────────────────────────────

#[test]
fn r5103_triggers_json_response_with_redundant_content_type() {
    let src = r#"
return JsonResponse(data, content_type="application/json")
"#;
    let codes = run_check(&RedundantContentTypeForJsonResponse, src);
    assert!(
        codes.contains(&"R5103".to_string()),
        "expected R5103, got {:?}",
        codes
    );
}

#[test]
fn r5103_no_trigger_json_response_without_content_type() {
    let src = r#"
return JsonResponse(data)
"#;
    let codes = run_check(&RedundantContentTypeForJsonResponse, src);
    assert!(
        !codes.contains(&"R5103".to_string()),
        "unexpected R5103, got {:?}",
        codes
    );
}

#[test]
fn r5103_no_trigger_http_response_with_content_type() {
    // R5103 is only about JsonResponse — HttpResponse is handled by R5102
    let src = r#"
return HttpResponse(data, content_type="application/json")
"#;
    let codes = run_check(&RedundantContentTypeForJsonResponse, src);
    assert!(
        !codes.contains(&"R5103".to_string()),
        "unexpected R5103 for HttpResponse, got {:?}",
        codes
    );
}

// ── W5197: MissingBackwardsMigrationCallable ──────────────────────────────

#[test]
fn w5197_triggers_run_python_without_reverse_code() {
    let src = r#"
RunPython(my_migration_func)
"#;
    let codes = run_check_with_filename(
        &MissingBackwardsMigrationCallable,
        src,
        "/app/migrations/0002_add_field.py",
    );
    assert!(
        codes.contains(&"W5197".to_string()),
        "expected W5197, got {:?}",
        codes
    );
}

#[test]
fn w5197_no_trigger_run_python_with_reverse_code() {
    let src = r#"
RunPython(my_migration_func, reverse_code=my_reverse_func)
"#;
    let codes = run_check_with_filename(
        &MissingBackwardsMigrationCallable,
        src,
        "/app/migrations/0002_add_field.py",
    );
    assert!(
        !codes.contains(&"W5197".to_string()),
        "unexpected W5197, got {:?}",
        codes
    );
}

#[test]
fn w5197_no_trigger_outside_migrations_directory() {
    // W5197 only runs on files in a migrations directory
    let src = r#"
RunPython(my_func)
"#;
    let codes = run_check(&MissingBackwardsMigrationCallable, src);
    assert!(
        !codes.contains(&"W5197".to_string()),
        "unexpected W5197 outside migrations, got {:?}",
        codes
    );
}

#[test]
fn w5197_no_trigger_run_python_with_two_positional_args() {
    // RunPython(forward, backward) — the second arg IS the reverse callable
    let src = r#"
RunPython(my_migration_func, my_reverse_func)
"#;
    let codes = run_check_with_filename(
        &MissingBackwardsMigrationCallable,
        src,
        "/app/migrations/0002_add_field.py",
    );
    assert!(
        !codes.contains(&"W5197".to_string()),
        "unexpected W5197 with two positional args, got {:?}",
        codes
    );
}

// ── W5198: NewDbFieldWithDefault ──────────────────────────────────────────

#[test]
fn w5198_triggers_add_field_with_default() {
    let src = r#"
AddField(model_name="mymodel", name="status", field=CharField(default="active"))
"#;
    let codes = run_check_with_filename(
        &NewDbFieldWithDefault,
        src,
        "/app/migrations/0003_add_status.py",
    );
    assert!(
        codes.contains(&"W5198".to_string()),
        "expected W5198, got {:?}",
        codes
    );
}

#[test]
fn w5198_no_trigger_add_field_without_default() {
    let src = r#"
AddField(model_name="mymodel", name="status", field=CharField())
"#;
    let codes = run_check_with_filename(
        &NewDbFieldWithDefault,
        src,
        "/app/migrations/0003_add_status.py",
    );
    assert!(
        !codes.contains(&"W5198".to_string()),
        "unexpected W5198, got {:?}",
        codes
    );
}

#[test]
fn w5198_no_trigger_outside_migrations_directory() {
    let src = r#"
AddField(model_name="mymodel", name="status", field=CharField(default="active"))
"#;
    let codes = run_check(&NewDbFieldWithDefault, src);
    assert!(
        !codes.contains(&"W5198".to_string()),
        "unexpected W5198 outside migrations, got {:?}",
        codes
    );
}
