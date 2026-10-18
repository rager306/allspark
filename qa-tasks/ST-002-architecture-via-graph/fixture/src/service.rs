use crate::repo::Repo;

pub fn handle(repo: &Repo) -> u32 {
    validate(repo.load())
}

fn validate(value: u32) -> u32 {
    value + 1
}
