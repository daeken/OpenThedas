// Usage: osascript -l JavaScript docs/launcher-qa/macos-ax.js <scratch-launcher-pid> [press]
// Reads only the specified process's single window. "press" presses its game-store
// disclosure through AXPress. A first query can activate AccessKit's placeholder
// tree; query again after the launcher has had a frame to publish the full tree.
ObjC.import('ApplicationServices');
// AXUIElementRef and CFTypeRef need explicit object bindings in the ObjC bridge.
ObjC.bindFunction('AXUIElementCreateApplication', ['id', ['int']]);
ObjC.bindFunction('AXUIElementCopyAttributeValue', ['int', ['id', 'id', 'id*']]);
ObjC.bindFunction('AXUIElementSetMessagingTimeout', ['int', ['id', 'float']]);
ObjC.bindFunction('AXUIElementPerformAction', ['int', ['id', 'id']]);

function getAttribute(element, name) {
    var value = Ref();
    var result = $.AXUIElementCopyAttributeValue(element, $(name), value);
    // A placeholder node may be destroyed while the initial tree is published.
    // The other two errors mean unsupported attribute and no value, respectively.
    if (result === -25202 || result === -25205 || result === -25212) return null;
    if (result !== 0) throw new Error(name + ' failed: ' + result);
    return value[0];
}

function textValue(element, name) {
    var value = getAttribute(element, name);
    return value === null ? '<missing>' : String(ObjC.deepUnwrap(value));
}

function run(argv) {
    var app = $.AXUIElementCreateApplication(Number(argv[0]));
    $.AXUIElementSetMessagingTimeout(app, 3);
    var windows = getAttribute(app, 'AXWindows');
    if (Number(windows.count) !== 1) throw new Error('Expected one scratch launcher window, got ' + windows.count);
    var rows = [];
    var pressed = false;
    function visit(element, depth) {
        if (depth > 30) throw new Error('Accessibility tree too deep');
        $.AXUIElementSetMessagingTimeout(element, 3);
        var role = textValue(element, 'AXRole');
        var title = textValue(element, 'AXTitle');
        if (argv.length > 1 && title === "Don't have the game?") {
            var result = $.AXUIElementPerformAction(element, $('AXPress'));
            if (result !== 0) throw new Error('AXPress failed: ' + result);
            pressed = true;
            return;
        }
        var row = depth + ' | AXRole=' + role + ' | AXTitle=' + title;
        if (title === "Don't have the game?") {
            row += ' | AXValue=' + textValue(element, 'AXValue');
            row += ' | AXExpanded=' + textValue(element, 'AXExpanded');
        }
        rows.push(row);
        var children = getAttribute(element, 'AXChildren');
        if (children !== null) {
            for (var i = 0; i < children.count; i++) {
                visit(children.objectAtIndex(i), depth + 1);
                if (pressed) return;
            }
        }
    }
    visit(windows.objectAtIndex(0), 0);
    return pressed ? "PRESSED Don't have the game?" : rows.join('\n');
}
