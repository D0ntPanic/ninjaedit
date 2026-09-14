//! Laying out the commit graph: which lane each commit's node sits in,
//! and how the lines between commits run, as `git log --graph` draws
//! them.
//!
//! Commits arrive newest first, every commit before its parents (a
//! topological order). The layout keeps a row of *lanes*, each waiting
//! for a commit: the one the line running down it reaches next. A commit
//! takes the first lane waiting for it (or a new lane, when nothing
//! above it has been shown: a branch tip); any other lanes waiting for
//! it join its lane there. Its first parent takes over its lane and each
//! further parent forks off into the lane already waiting for that
//! parent or into a new one, which is how a merge shows as a line
//! branching away below its node.
//!
//! Each commit is drawn as two lines. The node line has the commit's
//! node with the other lanes passing straight down beside it. The
//! transition line below it carries the lines from this row's lanes to
//! the next row's: straight where a lane continues, curving where lanes
//! join the next commit or fork from this one. The layout describes a
//! row as the lanes at its node line and the [edges](Edge) of its
//! transition line; [`GraphRow::node_cells`] and
//! [`GraphRow::transition_cells`] turn those into box-drawing glyphs,
//! each with the number of the lane's color, so that every frontend
//! draws the same graph.
//!
//! The joins into a commit are drawn on the transition line above it,
//! so a row is finished only when the next commit is known:
//! [`GraphLayout::push`] returns the row of the commit pushed *before*
//! the one given, and [`GraphLayout::finish`] the last.

/// A line of the transition below a commit's row, from a lane at this
/// row's node line to a lane at the next row's. Straight when `from`
/// and `to` are the same lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    /// The number of the line's color; see [`GraphRow::color`].
    pub color: usize,
}

/// One commit's place in the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane the commit's node is in.
    pub column: usize,
    /// The number of the node's color. Colors are numbered from zero in
    /// the order the lanes were started; a frontend takes the number
    /// modulo the size of its palette, so lanes near each other differ.
    pub color: usize,
    /// The lanes at the node line: for each lane, the color of the line
    /// running down it, or `None` where there is none. The node's own
    /// lane is included.
    pub lanes: Vec<Option<usize>>,
    /// The lines of the transition below the node line.
    pub edges: Vec<Edge>,
}

/// One glyph of a drawn graph line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphCell {
    pub glyph: &'static str,
    /// The color number of what is drawn, or `None` for a blank.
    pub color: Option<usize>,
    /// Whether this is the commit's node, which a frontend may draw
    /// with its own symbol.
    pub node: bool,
}

const BLANK: GraphCell = GraphCell {
    glyph: " ",
    color: None,
    node: false,
};

/// The symbol for a commit's node.
pub const NODE: &str = "●";

/// The sides of a cell a line reaches, as bits: up, down, left, right.
const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// The box-drawing glyph for the sides of a cell a line reaches.
fn glyph_for(bits: u8) -> &'static str {
    const VERTICAL: u8 = UP | DOWN;
    const HORIZONTAL: u8 = LEFT | RIGHT;
    const UP_RIGHT: u8 = UP | RIGHT;
    const UP_LEFT: u8 = UP | LEFT;
    const DOWN_RIGHT: u8 = DOWN | RIGHT;
    const DOWN_LEFT: u8 = DOWN | LEFT;
    const VERTICAL_RIGHT: u8 = VERTICAL | RIGHT;
    const VERTICAL_LEFT: u8 = VERTICAL | LEFT;
    const HORIZONTAL_DOWN: u8 = HORIZONTAL | DOWN;
    const HORIZONTAL_UP: u8 = HORIZONTAL | UP;
    match bits {
        0 => " ",
        UP => "╵",
        DOWN => "╷",
        LEFT => "╴",
        RIGHT => "╶",
        VERTICAL => "│",
        HORIZONTAL => "─",
        UP_RIGHT => "╰",
        UP_LEFT => "╯",
        DOWN_RIGHT => "╭",
        DOWN_LEFT => "╮",
        VERTICAL_RIGHT => "├",
        VERTICAL_LEFT => "┤",
        HORIZONTAL_DOWN => "┬",
        HORIZONTAL_UP => "┴",
        _ => "┼",
    }
}

impl GraphRow {
    /// How many lanes the row spans, counting the lanes its transition
    /// reaches.
    pub fn width(&self) -> usize {
        let edges = self
            .edges
            .iter()
            .map(|edge| edge.from.max(edge.to) + 1)
            .max()
            .unwrap_or(0);
        self.lanes.len().max(edges)
    }

    /// The glyphs of the node line, `lanes` lanes wide: two cells per
    /// lane (the lane and the gap after it), less the last gap.
    pub fn node_cells(&self, lanes: usize) -> Vec<GraphCell> {
        let mut cells = vec![BLANK; cells_for(lanes)];
        for (lane, color) in self.lanes.iter().enumerate().take(lanes) {
            if let Some(color) = color {
                let cell = &mut cells[lane * 2];
                if lane == self.column {
                    *cell = GraphCell {
                        glyph: NODE,
                        color: Some(*color),
                        node: true,
                    };
                } else {
                    *cell = GraphCell {
                        glyph: "│",
                        color: Some(*color),
                        node: false,
                    };
                }
            }
        }
        cells
    }

    /// The glyphs of the transition line below the node line, `lanes`
    /// lanes wide. Lines to lanes beyond that are cut off.
    pub fn transition_cells(&self, lanes: usize) -> Vec<GraphCell> {
        let count = cells_for(lanes);
        let mut bits = vec![0u8; count];
        let mut colors: Vec<Option<usize>> = vec![None; count];
        // Straight lines first, so that a line crossing them takes the
        // color of the crossing only where nothing ran straight through.
        let (straight, curved): (Vec<&Edge>, Vec<&Edge>) =
            self.edges.iter().partition(|edge| edge.from == edge.to);
        for edge in straight {
            let x = edge.from * 2;
            if x < count {
                bits[x] |= UP | DOWN;
                colors[x] = Some(edge.color);
            }
        }
        for edge in curved {
            let (from, to) = (edge.from * 2, edge.to * 2);
            let (left, right) = if from < to { (from, to) } else { (to, from) };
            for x in left..=right.min(count.saturating_sub(1)) {
                let mut side = 0;
                if x == from {
                    side |= UP;
                }
                if x == to {
                    side |= DOWN;
                }
                if x > left {
                    side |= LEFT;
                }
                if x < right {
                    side |= RIGHT;
                }
                bits[x] |= side;
                if colors[x].is_none() {
                    colors[x] = Some(edge.color);
                }
            }
        }
        bits.into_iter()
            .zip(colors)
            .map(|(bits, color)| GraphCell {
                glyph: glyph_for(bits),
                color: if bits == 0 { None } else { color },
                node: false,
            })
            .collect()
    }
}

/// The cells a graph `lanes` lanes wide takes: a lane and a gap each,
/// without the gap after the last.
pub fn cells_for(lanes: usize) -> usize {
    (lanes * 2).saturating_sub(1)
}

#[derive(Clone, Copy)]
struct Lane<T> {
    /// The commit the line in this lane reaches next.
    expects: T,
    color: usize,
}

/// The row of the commit last pushed, waiting for the next commit to
/// finish its transition line.
struct Pending {
    row: GraphRow,
    /// For each lane, whether a line was in it at the node line and
    /// continues below: those get a straight edge, or a join into the
    /// next commit.
    continuing: Vec<bool>,
}

/// The lane state of a graph being laid out commit by commit.
pub struct GraphLayout<T> {
    lanes: Vec<Option<Lane<T>>>,
    next_color: usize,
    pending: Option<Pending>,
}

impl<T: Copy + PartialEq> Default for GraphLayout<T> {
    fn default() -> Self {
        GraphLayout::new()
    }
}

impl<T: Copy + PartialEq> GraphLayout<T> {
    pub fn new() -> GraphLayout<T> {
        GraphLayout {
            lanes: Vec::new(),
            next_color: 0,
            pending: None,
        }
    }

    /// Lay out the next commit, given its parents in order. Returns the
    /// finished row of the commit pushed before it, if there was one.
    pub fn push(&mut self, id: T, parents: &[T]) -> Option<GraphRow> {
        // The lanes waiting for this commit: the first is its own, the
        // rest join it here.
        let matching: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.is_some_and(|lane| lane.expects == id))
            .map(|(index, _)| index)
            .collect();
        let (column, color) = match matching.first() {
            Some(&index) => (index, self.lanes[index].expect("matched").color),
            None => {
                let color = self.new_color();
                let index = self.allocate(Lane { expects: id, color });
                (index, color)
            }
        };
        for &index in matching.iter().skip(1) {
            self.lanes[index] = None;
        }
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }

        // The previous row's transition: every continuing lane runs
        // straight on, except those that join this commit.
        let finished = self.pending.take().map(|mut pending| {
            for (lane, _) in pending
                .continuing
                .iter()
                .enumerate()
                .filter(|(_, continuing)| **continuing)
            {
                let to = if matching.contains(&lane) {
                    column
                } else {
                    lane
                };
                pending.row.edges.push(Edge {
                    from: lane,
                    to,
                    color: pending.row.lanes[lane].unwrap_or(color),
                });
            }
            pending.row
        });

        let lanes: Vec<Option<usize>> = self
            .lanes
            .iter()
            .map(|lane| lane.map(|lane| lane.color))
            .collect();
        let mut continuing: Vec<bool> = self.lanes.iter().map(Option::is_some).collect();

        // The first parent carries the lane on; the others fork off.
        let mut edges = Vec::new();
        match parents.first() {
            Some(&parent) => {
                self.lanes[column] = Some(Lane {
                    expects: parent,
                    color,
                });
            }
            None => {
                self.lanes[column] = None;
                continuing[column] = false;
            }
        }
        for &parent in parents.iter().skip(1) {
            let existing = self
                .lanes
                .iter()
                .position(|lane| lane.is_some_and(|lane| lane.expects == parent));
            let (to, edge_color) = match existing {
                Some(index) => (index, self.lanes[index].expect("found").color),
                None => {
                    let edge_color = self.new_color();
                    let index = self.allocate(Lane {
                        expects: parent,
                        color: edge_color,
                    });
                    (index, edge_color)
                }
            };
            if to != column && !edges.iter().any(|edge: &Edge| edge.to == to) {
                edges.push(Edge {
                    from: column,
                    to,
                    color: edge_color,
                });
            }
        }
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }

        self.pending = Some(Pending {
            row: GraphRow {
                column,
                color,
                lanes,
                edges,
            },
            continuing,
        });
        finished
    }

    /// The row of the last commit pushed, once there are no more. Its
    /// lanes run straight off the bottom, which only happens when the
    /// history was cut short (a shallow clone).
    pub fn finish(&mut self) -> Option<GraphRow> {
        self.pending.take().map(|mut pending| {
            for (lane, _) in pending
                .continuing
                .iter()
                .enumerate()
                .filter(|(_, continuing)| **continuing)
            {
                pending.row.edges.push(Edge {
                    from: lane,
                    to: lane,
                    color: pending.row.lanes[lane].unwrap_or(0),
                });
            }
            pending.row
        })
    }

    fn new_color(&mut self) -> usize {
        let color = self.next_color;
        self.next_color += 1;
        color
    }

    /// Put a lane in the first free slot, or a new one at the right.
    fn allocate(&mut self, lane: Lane<T>) -> usize {
        match self.lanes.iter().position(Option::is_none) {
            Some(index) => {
                self.lanes[index] = Some(lane);
                index
            }
            None => {
                self.lanes.push(Some(lane));
                self.lanes.len() - 1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out commits given as `(id, parents)` and draw each as its two
    /// lines of text.
    fn draw(commits: &[(char, &str)]) -> Vec<String> {
        let mut layout = GraphLayout::new();
        let mut rows = Vec::new();
        for (id, parents) in commits {
            let parents: Vec<char> = parents.chars().collect();
            rows.extend(layout.push(*id, &parents));
        }
        rows.extend(layout.finish());
        assert_eq!(rows.len(), commits.len());
        let width = rows.iter().map(GraphRow::width).max().unwrap_or(0);
        let mut lines = Vec::new();
        for (row, (id, _)) in rows.iter().zip(commits) {
            let node: String = row
                .node_cells(width)
                .iter()
                .map(|cell| {
                    if cell.node {
                        id.to_string()
                    } else {
                        cell.glyph.to_owned()
                    }
                })
                .collect();
            let transition: String = row
                .transition_cells(width)
                .iter()
                .map(|cell| cell.glyph)
                .collect();
            lines.push(node.trim_end().to_owned());
            lines.push(transition.trim_end().to_owned());
        }
        lines
    }

    #[test]
    fn a_straight_line() {
        assert_eq!(
            draw(&[('c', "b"), ('b', "a"), ('a', "")]),
            ["c", "│", "b", "│", "a", ""]
        );
    }

    #[test]
    fn a_merge_forks_and_the_branch_joins_back() {
        // m merges b into c; both come from a.
        let lines = draw(&[('m', "cb"), ('c', "a"), ('b', "a"), ('a', "")]);
        assert_eq!(
            lines,
            [
                "m",   //
                "├─╮", //
                "c │", //
                "│ │", //
                "│ b", //
                "├─╯", //
                "a",   //
                "",
            ]
        );
    }

    #[test]
    fn a_second_branch_tip_starts_its_own_lane_and_joins() {
        // Two tips, t and h, with h on top and t's line joining h's at a.
        let lines = draw(&[('h', "a"), ('t', "a"), ('a', "")]);
        assert_eq!(lines, ["h", "│", "│ t", "├─╯", "a", ""]);
    }

    #[test]
    fn a_fork_into_a_waiting_lane_and_a_join_share_a_row() {
        // m merges b, whose lane already waits below t.
        let lines = draw(&[('t', "b"), ('m', "cb"), ('c', "a"), ('b', "a"), ('a', "")]);
        assert_eq!(
            lines,
            [
                "t",   //
                "│",   //
                "│ m", //
                "├─┤", //
                "│ c", //
                "│ │", //
                "b │", //
                "├─╯", //
                "a",   //
                "",
            ]
        );
        let mut layout = GraphLayout::new();
        layout.push('t', &['b']);
        let row = layout.push('m', &['c', 'b']).unwrap();
        assert_eq!(row.column, 0);
        assert_eq!(row.lanes, vec![Some(0)]);
        let row = layout.push('c', &['a']).unwrap();
        // m forks into the lane waiting for b (lane 0) and carries on
        // in its own.
        assert_eq!(row.column, 1);
        assert!(row.edges.contains(&Edge {
            from: 1,
            to: 0,
            color: 0
        }));
        assert!(row.edges.contains(&Edge {
            from: 0,
            to: 0,
            color: 0
        }));
    }

    #[test]
    fn colors_follow_the_lanes() {
        let mut layout = GraphLayout::new();
        layout.push('m', &['c', 'b']);
        let m = layout.push('c', &['a']).unwrap();
        assert_eq!(m.color, 0);
        assert_eq!(
            m.edges,
            vec![
                Edge {
                    from: 0,
                    to: 1,
                    color: 1
                },
                Edge {
                    from: 0,
                    to: 0,
                    color: 0
                }
            ]
        );
        let c = layout.push('b', &['a']).unwrap();
        assert_eq!(c.lanes, vec![Some(0), Some(1)]);
        let b = layout.push('a', &[]).unwrap();
        assert_eq!(b.column, 1);
        assert_eq!(b.color, 1);
        let cells = b.transition_cells(2);
        assert_eq!(cells[0].glyph, "├");
        assert_eq!(cells[0].color, Some(0), "the straight line keeps its color");
        assert_eq!(cells[1].glyph, "─");
        assert_eq!(cells[1].color, Some(1));
        assert_eq!(cells[2].glyph, "╯");
        assert_eq!(cells[2].color, Some(1));
        let a = layout.finish().unwrap();
        assert_eq!(a.lanes, vec![Some(0)]);
        assert!(a.edges.is_empty(), "a root ends its lane");
    }

    #[test]
    fn a_shallow_history_runs_its_lanes_off_the_bottom() {
        let lines = draw(&[('b', "a")]);
        assert_eq!(lines, ["b", "│"]);
    }

    #[test]
    fn lines_crossing_a_lane_draw_through_it() {
        // m forks to a new lane past a lane that runs straight through.
        let mut layout = GraphLayout::new();
        layout.push('t', &['x']);
        layout.push('m', &['c', 'b']);
        let m = layout.push('c', &['a']).unwrap();
        assert_eq!(m.column, 1);
        assert_eq!(m.lanes, vec![Some(0), Some(1)]);
        let cells = m.transition_cells(3);
        let text: String = cells.iter().map(|c| c.glyph).collect();
        assert_eq!(text, "│ ├─╮");
        // A fork leftwards into a freed lane, over a straight lane.
        let mut layout = GraphLayout::new();
        layout.push('t', &['x']);
        layout.push('u', &['y']);
        layout.push('v', &['z']);
        layout.push('x', &[]);
        layout.push('z', &['c', 'b']);
        let z = layout.push('c', &['a']).unwrap();
        assert_eq!(z.column, 2);
        assert_eq!(z.lanes, vec![None, Some(1), Some(2)]);
        let text: String = z.transition_cells(3).iter().map(|c| c.glyph).collect();
        assert_eq!(text, "╭─┼─┤");
    }

    #[test]
    fn a_wide_graph_is_cut_at_the_lanes_drawn() {
        let mut layout = GraphLayout::new();
        layout.push('m', &['c', 'b', 'd']);
        let m = layout.push('c', &['a']).unwrap();
        assert_eq!(m.width(), 3);
        assert_eq!(m.node_cells(1).len(), 1);
        let text: String = m.transition_cells(2).iter().map(|c| c.glyph).collect();
        assert_eq!(text, "├─┬");
    }
}
