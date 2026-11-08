// Chunky's command tree (`BrigadierCommand.build` in Chunky's common module): every sub-command and the words it takes,
// so the client completes them. A usage has no alternatives, so each fixed choice (a shape, a pattern, the `reload` and
// `trim` words) is a usage of its own.
fn chunky_register(commands: &mut Vec<crate::game::native_server::NativeCommand>) {
    const SHAPES: [&str; 9] = ["square", "circle", "rectangle", "ellipse", "diamond", "triangle", "pentagon", "hexagon", "star"];
    const PATTERNS: [&str; 7] = ["concentric", "loop", "spiral", "region", "world", "strip", "ring"];
    let mut usages: Vec<String> = [
        "chunky",
        "chunky help",
        "chunky help [page:integer]",
        "chunky start",
        "chunky start <world:word>",
        "chunky pause",
        "chunky pause <world:word>",
        "chunky continue",
        "chunky continue <world:word>",
        "chunky cancel",
        "chunky cancel <world:word>",
        "chunky progress",
        "chunky world",
        "chunky world <world:word>",
        "chunky worldborder",
        "chunky worldborder <world:word>",
        "chunky center",
        "chunky center <x:word> <z:word>",
        "chunky spawn",
        "chunky corners <x1:word> <z1:word> <x2:word> <z2:word>",
        "chunky radius <radius:word>",
        "chunky radius <radius:word> <radiusz:word>",
        "chunky silent",
        "chunky selection",
        "chunky quiet <interval:integer>",
        "chunky limit <rate:integer>",
        "chunky debug",
        "chunky debug map",
        "chunky reload",
        "chunky reload config",
        "chunky reload tasks",
        "chunky trim",
        "chunky trim abort",
        "chunky trim <world:word>",
        "chunky confirm",
        "chunky pattern csv <name:word>",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for &pattern in PATTERNS.iter() {
        usages.push(format!("chunky pattern {pattern}"));
    }
    for &shape in SHAPES.iter() {
        usages.push(format!("chunky shape {shape}"));
        usages.push(format!("chunky start <world:word> {shape} [x:word] [z:word] [rx:word] [rz:word]"));
        usages.push(format!("chunky trim <world:word> {shape} [x:word] [z:word] [rx:word] [rz:word]"));
        for side in ["inside", "outside"] {
            usages.push(format!("chunky trim <world:word> {shape} <x:word> <z:word> <rx:word> <rz:word> {side} [filters:text]"));
        }
    }
    for usage in &usages {
        commands.push(crate::game::chunky_glue::command_for(usage));
    }
}
