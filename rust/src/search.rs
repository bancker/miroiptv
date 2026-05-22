#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ItemKind {
    Live,
    Movie,
    Series,
}

#[derive(Debug, Clone)]
pub struct SearchItem {
    pub id: i64,
    pub name: String,
    pub kind: ItemKind,
}

pub fn rank<'a>(query: &str, items: &'a [SearchItem]) -> Vec<&'a SearchItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }

    let mut hits: Vec<&SearchItem> = items
        .iter()
        .filter(|it| it.name.to_lowercase().contains(&q))
        .collect();

    hits.sort_by(|a, b| {
        let ak = kind_rank(a.kind);
        let bk = kind_rank(b.kind);
        ak.cmp(&bk)
            .then_with(|| a.name.len().cmp(&b.name.len()))
            .then_with(|| a.name.cmp(&b.name))
    });

    hits.truncate(12);
    hits
}

fn kind_rank(k: ItemKind) -> u8 {
    match k {
        ItemKind::Live => 0,
        ItemKind::Movie => 1,
        ItemKind::Series => 2,
    }
}
