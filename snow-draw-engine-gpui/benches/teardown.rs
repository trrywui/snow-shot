use snow_draw_engine_gpui::{CanvasDocument, CanvasSession, engine::*};
use std::time::Instant;
fn main() {
    let output = std::env::args().nth(1).expect("JSON destination");
    let mut results = Vec::new();
    for (count, edits, text_bytes, samples) in [
        (10000, 0, 0, 5),
        (100000, 0, 0, 3),
        (5000, 5000, 0, 3),
        (1000, 0, 32768, 3),
    ] {
        let mut drop_ms = Vec::new();
        let mut construction_ms = Vec::new();
        for _ in 0..samples {
            let start = Instant::now();
            let mut doc = CanvasDocument::default();
            let mut session = CanvasSession::new(&mut doc).unwrap();
            session.set_surface(&mut doc, 960, 640).unwrap();
            let batch = if text_bytes > 0 { 8 } else { 128 };
            for base in (0..count).step_by(batch) {
                let ops:Vec<_>=(base..(base+batch).min(count)).map(|i|if text_bytes>0 {
                    serde_json::json!({"type":"text","bounds":[(i%100)*9,(i/100)*6,5,3],"text":"x".repeat(text_bytes)})
                } else {serde_json::json!({"type":"rectangle","bounds":[(i%100)*9,(i/100)*6,5,3]})}).collect();
                doc.command(|e| {
                    e.apply_annotation_json(
                        &serde_json::to_vec(&serde_json::json!({"version":1,"operations":ops}))
                            .unwrap(),
                    )
                })
                .unwrap();
            }
            session.refresh(&doc).unwrap();
            if edits > 0 {
                let id = session
                    .scene()
                    .ordered_nodes()
                    .map(|node| node.item.as_ref())
                    .find_map(|item| {
                        if let SceneDisplayItem::Rectangle(r) = item {
                            Some(ElementId {
                                index: r.id.index,
                                generation: r.id.generation,
                            })
                        } else {
                            None
                        }
                    })
                    .unwrap();
                session
                    .mutate(&mut doc, |e, v| {
                        e.select_element_with_viewport_changes(v, id)
                    })
                    .unwrap();
                for i in 0..edits {
                    session
                        .mutate(&mut doc, |e, v| {
                            e.set_selected_opacity_with_viewport_changes(
                                v,
                                if i % 2 == 0 { 0.5 } else { 1.0 },
                            )
                        })
                        .unwrap();
                }
            }
            construction_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            drop(session);
            let start = Instant::now();
            drop(doc);
            drop_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        println!("teardown elements={count}, edits={edits}, text bytes={text_bytes}: {drop_ms:?}");
        results.push(serde_json::json!({"elements":count,"extra_history_edits":edits,"text_bytes_per_item":text_bytes,"samples":samples,"construction_ms":construction_ms,"owner_thread_drop_ms":drop_ms}));
    }
    std::fs::write(output,serde_json::to_vec_pretty(&serde_json::json!({"profile":"canvas-perf Release opt3","machine":std::env::var("COMPUTERNAME").unwrap_or_default(),"fixtures":results,"absolute_worst_case":"not finite: the reused core has unbounded history and no incremental drain API"})).unwrap()).unwrap();
}
