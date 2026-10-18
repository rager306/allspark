mod service;
mod repo;

fn main() {
    let r = repo::Repo::new();
    service::handle(&r);
}
