use std::{cell::RefCell, fs, rc::Rc};

use thedas_python25::{Args, Error, NativeObject, ProcessOutput, Result, Runtime, Value};

#[test]
fn datetime_elapsed_time_and_timedelta_ordering() -> Result<()> {
    Runtime::new([]).run(
        r#"
import datetime
now = datetime.datetime.now()
elapsed = now - now
threshold = datetime.timedelta(seconds=5)
assert elapsed < threshold and threshold > elapsed
assert elapsed == datetime.timedelta() and not elapsed
assert threshold == datetime.timedelta(milliseconds=5000)
assert (threshold.days, threshold.seconds, threshold.microseconds) == (0, 5, 0)
before = datetime.timedelta(microseconds=-0.5)
assert (before.days, before.seconds, before.microseconds) == (-1, 86399, 999999)
assert before < elapsed and before
assert datetime.timedelta(seconds=0.00000025, microseconds=0.25).microseconds == 1
"#,
    )
}

#[test]
fn python2_expressions_and_control_flow() -> Result<()> {
    let mut runtime = Runtime::new([]);
    runtime.run(
        r#"
assert (-7 / 3, -7 % 3, 7 / 3.0) == (-3, 2, 2.3333333333333335)
assert (1 << 100) + 3 == 1267650600228229401496703205379L
assert ord('\xff'[0]) == 255
assert 'abc'[::-1] == 'cba'
assert str(['first', 'second']) == "['first', 'second']"
print('parenthesized print')
assert u'\u03b1'[0] == u'\u03b1'
assert not False and 1 < 2 <= 2
assert 2 not in [3, 4] and 2 <> 3
assert 'lm%02d %s' % (3, 'job') == 'lm03 job'
assert [x * 2 for x in range(5) if x % 2] == [2, 6]
assert x == 4
assert map(None, [1, 2], [3]) == [(1, 3), (2, None)]
items = map(lambda x: -x, xrange(4))
alias = items
items += [8]
assert alias is items
assert alias[1:] == [-1, -2, -3, 8]
acc = 0
for x in [1, 2, 3, 4]:
    if x == 2: continue
    if x == 4: break
    acc += x
else:
    assert False
assert acc == 4
while acc:
    acc -= 1
else:
    assert acc == 0
d = {'hello': 9}
assert d.get('hello') == 9
assert d['hello'] == 9
"#,
    )?;
    runtime.run("from __future__ import division\nassert 3 / 2 == 1.5\n")?;
    Ok(())
}

#[test]
fn lexical_scope_classes_and_old_exception_syntax() -> Result<()> {
    let mut runtime = Runtime::new([]);
    runtime.run(
        r#"
counter = 0
def make(initial):
    def shift(x=1, *extra, **options):
        global counter
        counter += 1
        return initial + x + len(extra) + options.get('delta', 0)
    return shift

class Light:
    brightness = 7
    def __init__(self, name):
        self.name = name
    def value(self):
        return self.brightness

class Spot(Light):
    brightness = 9

assert Spot('spot').value() == 9
assert make(12)(2, 3, **{'delta': 4}) == 19
assert make(12)() == 13
assert counter == 2

def fail():
    raise ValueError, 'broken'
n = 0
try:
    try:
        fail()
    except (IndexError, ValueError), err:
        assert str(err) == 'broken'
        raise
    finally:
        n = 9
except Exception, err:
    assert n == 9
    assert str(err) == 'broken'
else:
    assert False

def local():
    value = n
    n = 1
try:
    local()
except UnboundLocalError:
    pass
else:
    assert False
"#,
    )?;
    Ok(())
}

struct HostFlag(Rc<RefCell<i64>>);

impl NativeObject for HostFlag {
    fn get(&self, name: &str) -> Result<Value> {
        match name {
            "flag" => Ok(Value::int(*self.0.borrow())),
            _ => Err(Error::new("AttributeError", name)),
        }
    }

    fn set(&self, name: &str, value: Value) -> Result<()> {
        if name != "flag" {
            return Err(Error::new("AttributeError", name));
        }
        *self.0.borrow_mut() = value.as_i64()?;
        Ok(())
    }
}

#[test]
fn host_modules_binary_xml_and_commands() -> Result<()> {
    let mut runtime = Runtime::new([Value::bytes("--cpus=2"), Value::bytes("data")]);
    let flag = Rc::new(RefCell::new(2));
    runtime.register_module(
        "sample.child",
        Value::module([("value", Value::object(HostFlag(flag.clone())))]),
    )?;
    runtime.register_module("sample", Value::module([("marker", Value::int(8))]))?;
    runtime.set_process_runner(|argv, merged| {
        assert_eq!(argv, ["Helper.exe", "a b"]);
        assert!(merged);
        Ok(ProcessOutput {
            status: 3,
            stdout: b"hello".to_vec(),
            stderr: None,
        })
    });
    runtime.run(r#"
import sample.child
from sample.child import value as host_value
import struct, ctypes, getopt, sys, subprocess
import xml.dom.minidom
host_value.flag |= 4
assert sample.child.value.flag == 6
assert sample.marker == 8
assert struct.calcsize('bl') == 8
assert struct.unpack_from('>h', '\x00\xff\xfe', 1) == (-2,)
bits = ctypes.c_long(0xbf800000)
assert bits.value == -1082130432
assert struct.unpack('f', bits) == (-1.,)
bits.value = 0x100000000
assert bits.value == 0
opts, data = getopt.getopt(sys.argv, '', ['cpus='])
assert opts == [('--cpus', '2')]
assert data == ['data']
doc = xml.dom.minidom.parseString('<job><Light power="5">one</Light></job>')
light = doc.getElementsByTagName('Light')[0]
assert light.getAttribute('power') == '5'
assert light.childNodes[0].nodeValue == 'one'
si = subprocess.STARTUPINFO()
si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
proc = subprocess.Popen(['Helper.exe', 'a b'], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, startupinfo=si)
assert proc.returncode is None
assert proc.communicate() == ('hello', None)
assert proc.returncode == 3
proc = subprocess.Popen(['Helper.exe', 'a b'], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
assert proc.returncode is None
assert proc.wait() == 3
assert proc.returncode == 3
assert proc.wait() == 3
assert proc.poll() == 3
assert proc.communicate() == ('hello', None)
proc = subprocess.Popen(['Helper.exe', 'a b'], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
assert proc.returncode is None
assert proc.poll() == 3
assert proc.returncode == 3
"#)?;
    assert_eq!(*flag.borrow(), 6);
    Ok(())
}

#[test]
fn toolset_reads_resolve_mixed_case_asset_paths() -> Result<()> {
    let root = std::env::temp_dir().join(format!("thedas-python25-case-{}", std::process::id()));
    let models = root.join("ModelSource");
    fs::create_dir_all(&models).unwrap();
    let asset = models.join("prp_fertblsm_01.msh");
    fs::write(&asset, b"mesh").unwrap();
    let requested = root.join("modelsource").join("PRP_FerTblSm_01.msh");
    let mut runtime = Runtime::new([]);
    runtime.set_global("requested", Value::unicode(requested.to_str().unwrap()));
    if !requested.is_file() {
        runtime.run("import os.path\nassert not os.path.exists(requested)\n")?;
    }
    runtime.enable_case_insensitive_reads();
    runtime.run(
        "import os.path\nassert os.path.isfile(requested)\nassert open(requested, 'rb').read() == 'mesh'\n",
    )?;
    assert_eq!(runtime.resolve_read_path(&asset), asset);
    if !requested.is_file() {
        let other = models.join("PRP_FERTBLSM_01.MSH");
        fs::write(other, b"other").unwrap();
        assert_eq!(runtime.resolve_read_path(&requested), requested);
        assert_eq!(runtime.resolve_read_path(&asset), asset);
    }
    fs::remove_dir_all(root).unwrap();
    Ok(())
}

#[test]
fn unknown_syntax_and_exhausted_execution_cannot_pass() {
    let mut runtime = Runtime::new([]);
    let syntax = runtime.run("def (\n").unwrap_err();
    assert_eq!(syntax.class, "SyntaxError");
    let unknown = runtime
        .run("try:\n    {x for x in []}\nexcept:\n    pass\n")
        .unwrap_err();
    assert_eq!(unknown.class, "Unsupported");
    runtime.set_fuel(Some(20));
    let limit = runtime.run("while True: pass\n").unwrap_err();
    assert!(limit.message.contains("budget exhausted"));
}

#[test]
fn host_can_invoke_a_script_function() -> Result<()> {
    let mut runtime = Runtime::new([]);
    runtime.run("def f(x): return x * 2\n")?;
    let function = runtime.global("f").unwrap();
    let result = runtime.call(
        &function,
        Args {
            positional: vec![Value::int(9)],
            ..Args::default()
        },
    )?;
    assert_eq!(result.as_i64()?, 18);
    Ok(())
}

#[test]
fn source_imports_keep_module_globals_and_future_flags() -> Result<()> {
    use thedas_python25::ModuleSource;

    let mut runtime = Runtime::new([]);
    let loaded = Rc::new(RefCell::new(Vec::new()));
    let traced = loaded.clone();
    runtime.set_module_loader(move |name| {
        traced.borrow_mut().push(name.to_owned());
        let source = match name {
            "lighting" => "name = 'lighting'",
            "lighting.profile" => {
                "
from __future__ import division
import lighting
gain = 3
def scale(offset):
    global gain
    gain += offset
    return gain / 2
"
            }
            _ => return Ok(None),
        };
        Ok(Some(ModuleSource {
            source: source.to_owned(),
            filename: format!("{name}.py"),
        }))
    });
    runtime.run(
        "
import lighting.profile as profile
from lighting import profile as same_profile
assert profile is same_profile
assert profile.__name__ == 'lighting.profile'
assert profile.__file__ == 'lighting.profile.py'
gain = 20
assert profile.scale(2) == 2.5
assert profile.gain == 5
assert gain / 3 == 6
",
    )?;
    assert_eq!(*loaded.borrow(), ["lighting", "lighting.profile"]);
    Ok(())
}
