use super::*;

struct Empty;
impl Host for Empty {
    fn fscommand(&mut self, _: &str, _: &str) {}
    fn external_call(&mut self, _: &str, _: &[HostValue]) -> HostValue {
        HostValue::Undefined
    }
}
impl Fonts for Empty {
    fn font(&self, _: &str, _: bool, _: bool) -> Option<Arc<crate::swf::Font>> {
        None
    }
}

#[test]
fn nested_timeline_constructors_leave_the_last_descendant_for_load_callbacks() {
    let bytes = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
    let mut p = Player::new(
        bytes,
        "nested-classes",
        Box::new(Empty),
        Arc::new(Empty),
        None,
    )
    .unwrap();
    let placement = |character, depth, name: &str| {
        Command::Place(Box::new(Place {
            character: Some(character),
            depth,
            name: Some(name.into()),
            ..Default::default()
        }))
    };
    let timeline = |commands| {
        Character::Sprite(Arc::new(crate::swf::Timeline {
            frames: vec![crate::swf::Frame {
                commands,
                ..Default::default()
            }],
            ..Default::default()
        }))
    };
    let mut movie = Movie::default();
    movie.characters.insert(
        1,
        timeline(vec![placement(2, 1, "button"), placement(2, 2, "picker")]),
    );
    movie
        .characters
        .insert(2, timeline(vec![placement(3, 1, "label")]));
    movie.characters.insert(3, timeline(vec![]));
    p.vm.movie = Arc::new(movie);
    let proto = p.vm.alloc(Some(p.vm.protos.movie_clip), Default::default());
    let ctor = p.vm.define_native(p.vm.global, "Element", |vm, this, _| {
        let path = vm.display.path(vm.display_of(&this).unwrap(), true);
        vm.log.push(format!("construct {path}"));
        vm.set(&Value::Object(vm.global), "lastCreated", this);
        Value::Undefined
    });
    p.vm.define(ctor, "prototype", Value::Object(proto), 0);
    for id in 1..=3 {
        p.vm.classes.insert(id, Value::Object(ctor));
    }
    let root = p.vm.root;
    place(
        &mut p.vm,
        root,
        &Place {
            character: Some(1),
            depth: 1,
            name: Some("dialog".into()),
            ..Default::default()
        },
    );
    p.run_queue();
    let paths = [
        "_level0.dialog",
        "_level0.dialog.button",
        "_level0.dialog.button.label",
        "_level0.dialog.picker",
        "_level0.dialog.picker.label",
    ];
    let expected: Vec<String> = paths
        .iter()
        .map(|path| format!("construct {path}"))
        .collect();
    assert_eq!(p.drain_log(), expected);
    // GUI frameworks use this last-created instance to register callbacks that must
    // run on a particular descendant's load, before later siblings relayout the box.
    let global = Value::Object(p.vm.global);
    let last = p.vm.get(&global, "lastCreated");
    let last_path = p.vm.display.path(p.vm.display_of(&last).unwrap(), true);
    assert_eq!(last_path, "_level0.dialog.picker.label");
}
