#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Command,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub kind: Kind,
    pub title: String,
    pub detail: String,
    pub target: String,
}
