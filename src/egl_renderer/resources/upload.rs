/// Reusable CPU conversion storage shared by every ordinary texture upload.
#[derive(Default)]
pub(super) struct UploadScratch {
    pub(super) rgba: Vec<u8>,
}
