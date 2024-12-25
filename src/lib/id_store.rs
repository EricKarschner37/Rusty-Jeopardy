use rand::seq::SliceRandom;

#[derive(Debug)]
pub struct IdStore {
    unused_ids: Vec<String>,
}

impl IdStore {
    pub fn new() -> Self {
        let prefixes = vec![
            "smarty", "dummy", "stinky", "enormous", "smelly", "bright", "handsome", "silly",
            "whiny", "tall", "short", "wily", "clever",
        ];
        let suffixes = vec![
            "rat",
            "boy",
            "girl",
            "eric",
            "olivia",
            "michael",
            "liv",
            "livvy",
            "mike",
            "connor",
            "savvy",
            "mom",
            "dad",
            "alex-trebek",
            "ken-jennings",
            "datadog",
            "oscar",
            "abby",
            "lucy",
            "oliver",
            "oscar-oliver",
        ];

        let mut ids: Vec<String> = prefixes
            .into_iter()
            .map(|prefix| {
                suffixes
                    .iter()
                    .map(move |suffix| format!("{prefix}-{suffix}"))
            })
            .flatten()
            .collect();

        ids.shuffle(&mut rand::thread_rng());

        Self { unused_ids: ids }
    }

    pub fn take(&mut self) -> Option<String> {
        self.unused_ids.pop()
    }
}
