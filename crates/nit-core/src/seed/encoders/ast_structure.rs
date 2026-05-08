use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, hilbert_index_to_xy, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use super::structural::{ast_node_class, seed_parse};

pub(crate) struct AstStructureEncoder;

impl SeedEncoder for AstStructureEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::AstStructure
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order;
        let total = size * size;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();

        if bytes.is_empty() {
            return grid;
        }

        match seed_parse(input.text, input.file_path) {
            Some((tree, _lang)) => fill_grid_from_ast(&mut grid, &tree, total, order),
            None => fill_grid_neutral(&mut grid, total, order),
        }

        normalize_grid(&mut grid);
        apply_structural_noise(&mut grid, size, seed_nonce, bytes, variant);
        grid
    }
}

fn fill_grid_from_ast(
    grid: &mut SeedValueGrid,
    tree: &tree_sitter::Tree,
    total: usize,
    order: u32,
) {
    let mut node_fills: Vec<(u8, u32)> = Vec::new();
    let mut stack = vec![(tree.root_node(), 0u32, 0u32)];

    while let Some((node, depth, _sibling_index)) = stack.pop() {
        if node.is_named() {
            let child_count = node.named_child_count() as u32;
            let byte_span = (node.end_byte().saturating_sub(node.start_byte())) as u32;

            let depth_score = (depth.min(15) as f32 / 15.0 * 255.0) as u64;
            let branch_score = (child_count.min(20) as f32 / 20.0 * 255.0) as u64;
            let span_score = (byte_span.min(2000) as f32 / 2000.0 * 255.0) as u64;
            let kind_class = ast_node_class(node.kind()) as u64;

            let value = ((depth_score * 30 + branch_score * 25 + span_score * 25 + kind_class * 20)
                / 100)
                .clamp(0, 255) as u8;

            node_fills.push((value, byte_span));
        }

        let child_count = node.child_count();
        let mut named_idx = 0u32;
        for i in (0..child_count).rev() {
            if let Some(child) = node.child(i) {
                stack.push((child, depth + 1, named_idx));
                if child.is_named() {
                    named_idx += 1;
                }
            }
        }
    }

    let total_span: u32 = node_fills.iter().map(|(_, s)| *s).sum();
    let total_span = total_span.max(1) as f32;
    let mut cell_idx = 0usize;

    for (value, byte_span) in &node_fills {
        let cells = ((*byte_span as f32 / total_span) * total as f32)
            .round()
            .max(1.0) as usize;
        for _ in 0..cells {
            if cell_idx >= total {
                break;
            }
            let (x, y) = hilbert_index_to_xy(order, cell_idx as u32);
            grid.set(x as usize, y as usize, *value);
            cell_idx += 1;
        }
    }

    let fill = node_fills.last().map(|(v, _)| *v).unwrap_or(128);
    while cell_idx < total {
        let (x, y) = hilbert_index_to_xy(order, cell_idx as u32);
        grid.set(x as usize, y as usize, fill);
        cell_idx += 1;
    }
}

// Without tree-sitter we cannot extract meaningful AST structure, so we
// produce a deterministic but moderate signal rather than byte-level noise.
fn fill_grid_neutral(grid: &mut SeedValueGrid, total: usize, order: u32) {
    for cell in 0..total {
        let (x, y) = hilbert_index_to_xy(order, cell as u32);
        grid.set(x as usize, y as usize, 128);
    }
}
