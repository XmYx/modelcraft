// Dependency report: lists what each object's evaluation reads, from the dependency graph.
let edges = graph_edges().filter(|e| e.kind != "transform");
if edges.len() == 0 {
    print("no object reads another object's result");
}
for e in edges {
    let how = if e.kind == "deformer" { "is bent by" } else { "is built from" };
    print(`${name_of(e.from)} ${how} ${name_of(e.to)}`);
}
edges.len()
