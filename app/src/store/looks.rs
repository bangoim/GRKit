//! Film Looks of the connected camera: the list, changes, the official
//! catalog, pictures, and imports. The camera is authoritative: every change
//! ends with a fresh read of its Looks, also after a failure.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

use nsh_base::alert_dialog;
use nsh_core::picker::FilePickRequest;
use nsh_core::prelude::*;

use super::{AppState, Route, Tab};
use crate::domain::{
    CatalogLook, ConnectionState, JournalState, Look, LookError, LookFilter, LookGroup,
    LooksSnapshot,
    fake_looks::{FakeLooks, demo_catalog},
    group_of,
    look_media::{self, LookPreview, Picture},
    looks::{import_id, name_from_file},
    valid_name,
};
use crate::platform::{
    self,
    looks::{self as remote, CameraContext},
};
use crate::strings::tr;

/// The demo catalog: each official Look with its GRF file.
type DemoCatalog = Vec<(CatalogLook, Vec<u8>)>;

/// Which list the Looks tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LooksView {
    OnCamera,
    All,
}

/// An official Look of the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogItem {
    pub look: CatalogLook,
    pub group: LookGroup,
}

/// What shows for one Look content: the sample photo in it, or its menu icon
/// while the camera's base gamma is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookPicture {
    Preview(LookPreview),
    Icon(Picture),
}

impl LookPicture {
    pub fn thumb(&self) -> &Picture {
        match self {
            LookPicture::Preview(preview) => &preview.thumb,
            LookPicture::Icon(icon) => icon,
        }
    }

    pub fn full(&self) -> &Picture {
        match self {
            LookPicture::Preview(preview) => &preview.full,
            LookPicture::Icon(icon) => icon,
        }
    }
}

/// A Look being made from a file on the phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportDraft {
    pub file_name: String,
    pub id: String,
    pub name: String,
    /// The imported Look this replaces: its ID and content hash.
    pub replacing: Option<(String, String)>,
    pub working: bool,
    pub ready: bool,
    pub sending: bool,
    pub error: Option<LookError>,
}

/// Where the bytes of one Look content come from.
#[derive(Clone, Debug)]
enum Source {
    Camera { id: String },
    Catalog { look: CatalogLook },
}

/// Looks state, kept in the app state (`Copy` handles).
#[derive(Clone, Copy)]
pub struct LooksStore {
    snapshot: Signal<Option<LooksSnapshot>>,
    loading: Signal<bool>,
    error: Signal<Option<LookError>>,
    /// Why changes are not possible now (an update waits on the camera).
    blocked: Signal<Option<LookError>>,
    /// The Look (or `order`) being changed.
    busy: Signal<Option<String>>,
    view: Signal<LooksView>,
    filter: Signal<LookFilter>,
    catalog: Signal<Option<Vec<CatalogItem>>>,
    catalog_error: Signal<Option<String>>,
    catalog_target: Signal<Option<String>>,
    adding: Signal<BTreeSet<String>>,
    base_gamma: Stored<RefCell<Option<Vec<u16>>>>,
    /// Pictures by content hash; `pictures_rev` tells the UI that one arrived.
    pictures: Stored<RefCell<BTreeMap<String, LookPicture>>>,
    pictures_pending: Stored<RefCell<BTreeSet<String>>>,
    pictures_rev: Signal<u64>,
    draft: Signal<Option<ImportDraft>>,
    draft_grf: Stored<RefCell<Option<Vec<u8>>>>,
    draft_picture: Signal<Option<LookPicture>>,
    /// Menu order being edited.
    order: Signal<Vec<String>>,
    fake: Stored<RefCell<Option<FakeLooks>>>,
    fake_catalog: Stored<RefCell<DemoCatalog>>,
}

impl LooksStore {
    pub fn new(demo: bool) -> Self {
        Self {
            snapshot: signal(None),
            loading: signal(false),
            error: signal(None),
            blocked: signal(None),
            busy: signal(None),
            view: signal(LooksView::OnCamera),
            filter: signal(LookFilter::All),
            catalog: signal(None),
            catalog_error: signal(None),
            catalog_target: signal(None),
            adding: signal(BTreeSet::new()),
            base_gamma: stored(RefCell::new(demo.then(look_media::demo_base_gamma))),
            pictures: stored(RefCell::new(BTreeMap::new())),
            pictures_pending: stored(RefCell::new(BTreeSet::new())),
            pictures_rev: signal(0),
            draft: signal(None),
            draft_grf: stored(RefCell::new(None)),
            draft_picture: signal(None),
            order: signal(Vec::new()),
            fake: stored(RefCell::new(demo.then(FakeLooks::demo))),
            fake_catalog: stored(RefCell::new(if demo { demo_catalog() } else { Vec::new() })),
        }
    }
}

/// Run `work` on a worker thread and await its result.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    rx.recv().await.ok()
}

fn picture_for(grf: &[u8], base_gamma: Option<&[u16]>) -> Option<LookPicture> {
    if let Some(base_gamma) = base_gamma
        && let Ok(preview) = look_media::render_preview(grf, base_gamma)
    {
        return Some(LookPicture::Preview(preview));
    }
    look_media::icon(grf).ok().map(LookPicture::Icon)
}

impl AppState {
    fn looks_store(self) -> LooksStore {
        self.looks
    }

    pub fn looks_snapshot(self) -> Option<LooksSnapshot> {
        self.looks.snapshot.get()
    }

    pub fn looks_loading(self) -> bool {
        self.looks.loading.get()
    }

    pub fn looks_error(self) -> Option<LookError> {
        self.looks.error.get()
    }

    pub fn looks_blocked(self) -> Option<LookError> {
        self.looks.blocked.get()
    }

    pub fn look(self, id: &str) -> Option<Look> {
        self.looks
            .snapshot
            .get()
            .and_then(|snapshot| snapshot.get(id).cloned())
    }

    pub fn look_busy(self) -> Option<String> {
        self.looks.busy.get()
    }

    pub fn looks_view(self) -> LooksView {
        self.looks.view.get()
    }

    pub fn set_looks_view(self, view: LooksView) {
        self.looks.view.set(view);
        if view == LooksView::All {
            self.load_catalog();
        }
    }

    pub fn looks_filter(self) -> LookFilter {
        self.looks.filter.get()
    }

    pub fn set_looks_filter(self, filter: LookFilter) {
        self.looks.filter.set(filter);
    }

    pub fn catalog_items(self) -> Option<Vec<CatalogItem>> {
        self.looks.catalog.get()
    }

    pub fn catalog_error(self) -> Option<String> {
        self.looks.catalog_error.get()
    }

    pub fn catalog_adding(self, id: &str) -> bool {
        self.looks.adding.get().contains(id)
    }

    /// The picture of a Look content, when it is ready.
    pub fn look_picture(self, content_hash: &str) -> Option<LookPicture> {
        let _ = self.looks.pictures_rev.get();
        self.looks
            .pictures
            .with(|pictures| pictures.borrow().get(content_hash).cloned())
    }

    pub fn import_draft(self) -> Option<ImportDraft> {
        self.looks.draft.get()
    }

    pub fn import_picture(self) -> Option<LookPicture> {
        self.looks.draft_picture.get()
    }

    /// The camera menu icon of the Look being imported, drawn from the
    /// name as it is now, as sending will draw it.
    pub fn import_cover(self) -> Option<Picture> {
        let name = self.looks.draft.get()?.name;
        let name = name.trim();
        if !valid_name(name) {
            return None;
        }
        let icon =
            grkit_core::effects::icon_pixels(&crate::domain::looks::cover_for_name(name)).ok()?;
        Some(Picture {
            width: 40,
            height: 40,
            rgba: icon.into(),
        })
    }

    /// The camera menu icon of a Look on the camera, when its file is cached.
    pub fn look_icon(self, id: &str) -> Option<Picture> {
        let _ = self.looks.pictures_rev.get();
        if self.demo() {
            return self
                .looks
                .fake
                .with(|f| f.borrow().as_ref().and_then(|f| f.content(id)))
                .and_then(|grf| look_media::icon(&grf).ok());
        }
        let hash = self.look(id)?.content_hash;
        remote::cached(&hash).and_then(|grf: Vec<u8>| look_media::icon(&grf).ok())
    }

    pub fn editing_order(self) -> Vec<String> {
        self.looks.order.get()
    }

    fn connected(self) -> bool {
        self.status
            .get()
            .is_some_and(|s| s.camera.connection == ConnectionState::Connected)
    }

    /// The Looks tab became visible: read the camera's Looks.
    pub fn open_looks(self) {
        if self.connected() && !self.looks.loading.get() {
            self.refresh_looks();
        }
    }

    /// Read the camera's Looks, then load their pictures.
    pub fn refresh_looks(self) {
        if !self.connected() {
            return;
        }
        if self.demo() {
            let snapshot = self
                .looks
                .fake
                .with(|fake| fake.borrow().as_ref().map(FakeLooks::snapshot));
            let blocked = self.looks.fake.with(|fake| {
                fake.borrow()
                    .as_ref()
                    .is_some_and(|f| f.update_pending)
                    .then_some(LookError::UpdatePending)
            });
            self.looks.blocked.set(blocked);
            self.looks.snapshot.set(snapshot);
            self.looks.error.set(None);
            self.load_camera_pictures();
            return;
        }
        let generation = self.connect_generation.get();
        self.looks.loading.set(true);
        self.root.run(|| {
            spawn(async move {
                let result = remote::list().await;
                let journal = platform::update::snapshot().await.map(|s| s.state);
                if self.connect_generation.get() != generation {
                    return;
                }
                self.looks.loading.set(false);
                self.looks.blocked.set(
                    matches!(journal, Ok(JournalState::Waiting | JournalState::Confirmed))
                        .then_some(LookError::UpdatePending),
                );
                match result {
                    Ok(snapshot) => {
                        self.looks.snapshot.set(Some(snapshot));
                        self.looks.error.set(None);
                        self.load_camera_pictures();
                    }
                    Err(error) => {
                        log::warn!("Could not read Looks: {error:?}");
                        self.looks.error.set(Some(error));
                    }
                }
            });
        });
    }

    fn load_camera_pictures(self) {
        let Some(snapshot) = self.looks.snapshot.get() else {
            return;
        };
        let items = snapshot
            .looks
            .into_iter()
            .map(|look| (look.content_hash.clone(), Source::Camera { id: look.id }))
            .collect();
        self.load_pictures(items);
    }

    /// Fetch and draw the pictures that are missing, one at a time. The
    /// official catalog supplies the base gamma that previews need.
    fn load_pictures(self, items: Vec<(String, Source)>) {
        let wanted: Vec<(String, Source)> = items
            .into_iter()
            .filter(|(hash, _)| {
                let shown = self
                    .looks
                    .pictures
                    .with(|p| matches!(p.borrow().get(hash), Some(LookPicture::Preview(_))));
                !shown
                    && self
                        .looks
                        .pictures_pending
                        .with(|p| p.borrow_mut().insert(hash.clone()))
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.root.run(|| {
            spawn(async move {
                if self.looks.base_gamma.with(|b| b.borrow().is_none()) {
                    self.ensure_catalog().await;
                }
                let base_gamma = self.looks.base_gamma.with(|b| b.borrow().clone());
                for (hash, source) in wanted {
                    let bytes = self.source_bytes(&source).await;
                    let picture = match bytes {
                        Some(grf) => {
                            let base_gamma = base_gamma.clone();
                            off_thread(move || picture_for(&grf, base_gamma.as_deref()))
                                .await
                                .flatten()
                        }
                        None => None,
                    };
                    self.looks
                        .pictures_pending
                        .with(|p| p.borrow_mut().remove(&hash));
                    if let Some(picture) = picture {
                        self.looks
                            .pictures
                            .with(|p| p.borrow_mut().insert(hash, picture));
                        self.looks.pictures_rev.update(|rev| *rev += 1);
                    }
                }
            });
        });
    }

    async fn source_bytes(self, source: &Source) -> Option<Vec<u8>> {
        match source {
            Source::Camera { id } if self.demo() => self
                .looks
                .fake
                .with(|fake| fake.borrow().as_ref().and_then(|f| f.content(id))),
            Source::Camera { id } => {
                let hash = self.look(id).map(|look| look.content_hash)?;
                remote::content(id, &hash).await.ok()
            }
            Source::Catalog { look } if self.demo() => self.looks.fake_catalog.with(|c| {
                c.borrow()
                    .iter()
                    .find(|(l, _)| l.id == look.id)
                    .map(|(_, grf)| grf.clone())
            }),
            Source::Catalog { look } => {
                let target = self.looks.catalog_target.get()?;
                remote::catalog_file(&target, look).await.ok()
            }
        }
    }

    /// Load the official catalog for the camera firmware.
    async fn ensure_catalog(self) {
        if self.looks.catalog.get().is_some() {
            return;
        }
        if self.demo() {
            let items = self.looks.fake_catalog.with(|c| {
                c.borrow()
                    .iter()
                    .map(|(look, _)| CatalogItem {
                        group: group_of(&look.id, look.monochrome),
                        look: look.clone(),
                    })
                    .collect()
            });
            self.looks.catalog.set(Some(items));
            self.load_catalog_pictures();
            return;
        }
        let Some(status) = self.status.get() else {
            return;
        };
        let Some(target) = remote::target(status.camera.model, &status.camera.firmware) else {
            self.looks
                .catalog_error
                .set(Some(tr("looks.catalog_unsupported", &[])));
            return;
        };
        match remote::catalog(target).await {
            Ok(catalog) => {
                self.looks
                    .base_gamma
                    .with(|b| *b.borrow_mut() = Some(catalog.base_gamma.clone()));
                self.looks.catalog_target.set(Some(target.to_owned()));
                self.looks.catalog_error.set(None);
                self.looks.catalog.set(Some(
                    catalog
                        .looks
                        .into_iter()
                        .map(|look| CatalogItem {
                            group: group_of(&look.id, look.monochrome),
                            look,
                        })
                        .collect(),
                ));
                self.load_catalog_pictures();
            }
            Err(error) => {
                log::warn!("Could not load the Look catalog: {error}");
                self.looks
                    .catalog_error
                    .set(Some(tr("looks.catalog_offline", &[])));
            }
        }
    }

    /// Show the All Looks list: load the catalog, which loads its pictures.
    pub fn load_catalog(self) {
        if self.looks.catalog.get().is_some() {
            self.load_catalog_pictures();
            return;
        }
        self.root.run(|| {
            spawn(async move {
                self.ensure_catalog().await;
            });
        });
    }

    /// Queue the pictures of the catalog Looks that are not shown yet.
    fn load_catalog_pictures(self) {
        if let Some(items) = self.looks.catalog.get() {
            self.load_pictures(
                items
                    .into_iter()
                    .map(|item| {
                        (
                            item.look.sha256.clone(),
                            Source::Catalog { look: item.look },
                        )
                    })
                    .collect(),
            );
        }
    }

    /// Present a failed change as an alert.
    pub fn alert_look_error(self, error: LookError) {
        let (title, body) = error.keys();
        let body = match &error {
            LookError::Service(detail) => format!("{} {detail}", tr(body, &[])),
            _ => tr(body, &[]),
        };
        if matches!(
            error,
            LookError::UpdatePending | LookError::ChangesOff | LookError::LiveChangesUnavailable
        ) {
            self.looks.blocked.set(Some(error.clone()));
        }
        spawn(async move {
            let _ = alert_dialog()
                .title(tr(title, &[]))
                .description(body)
                .cancel("ok", tr("looks.ok", &[]))
                .show()
                .await;
        });
    }

    /// Apply one change: on the demo camera directly, on a real camera over
    /// HTTP. Reads the Looks again either way.
    fn change<F, Fut>(
        self,
        key: String,
        demo: impl FnOnce(&mut FakeLooks) -> Result<(), LookError>,
        real: F,
        settle: impl FnOnce(AppState, bool) + 'static,
    ) where
        F: FnOnce(CameraContext) -> Fut + 'static,
        Fut: std::future::Future<Output = Result<(), LookError>> + 'static,
    {
        if self.looks.busy.get().is_some() {
            return;
        }
        if self.demo() {
            let result = self.looks.fake.with(|fake| {
                fake.borrow_mut()
                    .as_mut()
                    .map_or(Err(LookError::ConnectionLost), demo)
            });
            self.refresh_looks();
            let ok = result.is_ok();
            if let Err(error) = result {
                self.alert_look_error(error);
            }
            settle(self, ok);
            return;
        }
        let Some(snapshot) = self.looks.snapshot.get() else {
            return;
        };
        let context = CameraContext::from(&snapshot);
        self.looks.busy.set(Some(key));
        self.root.run(|| {
            spawn(async move {
                let result = real(context).await;
                self.looks.busy.set(None);
                self.refresh_looks();
                let ok = result.is_ok();
                if let Err(error) = result {
                    self.alert_look_error(error);
                }
                settle(self, ok);
            });
        });
    }

    pub fn set_look_enabled(self, id: String, enabled: bool) {
        let key = id.clone();
        let demo_id = id.clone();
        self.change(
            key,
            move |fake| fake.set_enabled(&demo_id, enabled),
            move |context| async move { remote::set_enabled(&context, &id, enabled).await },
            |_, _| {},
        );
    }

    pub fn rename_look(self, id: String, name: String) {
        let name = name.trim().to_owned();
        if !valid_name(&name) {
            self.alert_look_error(LookError::InvalidName);
            return;
        }
        let (demo_id, demo_name) = (id.clone(), name.clone());
        self.change(
            id.clone(),
            move |fake| fake.rename(&demo_id, &demo_name),
            move |context| async move { remote::rename(&context, &id, &name).await },
            |app, ok| {
                if ok {
                    app.pop_to(Tab::Looks, 1);
                }
            },
        );
    }

    pub fn delete_look(self, id: String, active_off: bool) {
        let demo_id = id.clone();
        self.change(
            id.clone(),
            move |fake| fake.delete(&demo_id, active_off),
            move |context| async move { remote::delete(&context, &id, active_off).await },
            |app, ok| {
                if ok {
                    app.pop_to(Tab::Looks, 0);
                }
            },
        );
    }

    pub fn open_look_order(self) {
        if let Some(snapshot) = self.looks.snapshot.get() {
            self.looks.order.set(snapshot.ids());
            self.push(Route::LookOrder);
        }
    }

    pub fn move_look_in_order(self, from: usize, to: usize) {
        self.looks
            .order
            .update(|ids| crate::domain::looks::move_id(ids, from, to));
    }

    pub fn save_look_order(self) {
        let ids = self.looks.order.get();
        let unchanged = self
            .looks
            .snapshot
            .get()
            .is_some_and(|snapshot| snapshot.ids() == ids);
        if unchanged {
            self.pop_to(Tab::Looks, 0);
            return;
        }
        let demo_ids = ids.clone();
        self.change(
            "order".into(),
            move |fake| fake.set_order(&demo_ids),
            move |context| async move { remote::set_order(&context, &ids).await },
            |app, ok| {
                if ok {
                    app.pop_to(Tab::Looks, 0);
                }
            },
        );
    }

    /// Add an official Look from the catalog.
    pub fn add_catalog_look(self, id: String) {
        if self
            .looks
            .snapshot
            .get()
            .is_some_and(|snapshot| !snapshot.has_room())
        {
            self.alert_look_error(LookError::NoRoom);
            return;
        }
        let Some(item) = self
            .looks
            .catalog
            .get()
            .and_then(|items| items.into_iter().find(|item| item.look.id == id))
        else {
            return;
        };
        if self.looks.busy.get().is_some() {
            return;
        }
        self.looks.adding.update(|adding| {
            adding.insert(id.clone());
        });
        self.root.run(|| {
            spawn(async move {
                let source = Source::Catalog {
                    look: item.look.clone(),
                };
                let Some(grf) = self.source_bytes(&source).await else {
                    self.looks.adding.update(|a| {
                        a.remove(&id);
                    });
                    self.alert_look_error(LookError::Service(tr("looks.catalog_offline", &[])));
                    return;
                };
                let finish = move |app: AppState, _ok: bool| {
                    app.looks.adding.update(|a| {
                        a.remove(&item.look.id);
                    });
                };
                let demo_grf = grf.clone();
                let look_id = id.clone();
                self.change(
                    id.clone(),
                    move |fake| fake.put(&demo_grf, true),
                    move |context| async move {
                        remote::put_content(&context, &look_id, grf, None).await
                    },
                    finish,
                );
            });
        });
    }

    fn taken_ids(self) -> BTreeSet<String> {
        let mut taken: BTreeSet<String> = self
            .looks
            .snapshot
            .get()
            .map(|s| s.ids().into_iter().collect())
            .unwrap_or_default();
        if let Some(items) = self.looks.catalog.get() {
            taken.extend(items.into_iter().map(|item| item.look.id));
        }
        taken
    }

    /// Make an imported copy of an official Look, which can be renamed.
    pub fn copy_look(self, id: String) {
        let Some(look) = self.look(&id) else {
            return;
        };
        if self.looks.snapshot.get().is_some_and(|s| !s.has_room()) {
            self.alert_look_error(LookError::NoRoom);
            return;
        }
        let copy_id = import_id(&look.name, &self.taken_ids());
        self.root.run(|| {
            spawn(async move {
                let Some(grf) = self.source_bytes(&Source::Camera { id: id.clone() }).await else {
                    self.alert_look_error(LookError::ConnectionLost);
                    return;
                };
                let copy = match look_media::copy_as(&grf, &copy_id, &look.name) {
                    Ok(copy) => copy,
                    Err(error) => {
                        self.alert_look_error(LookError::Service(error.to_string()));
                        return;
                    }
                };
                let demo_copy = copy.clone();
                let open_id = copy_id.clone();
                self.change(
                    copy_id.clone(),
                    move |fake| fake.put(&demo_copy, false),
                    move |context| async move {
                        remote::put_content(&context, &copy_id, copy, None).await
                    },
                    move |app, ok| {
                        if ok {
                            app.pop_to(Tab::Looks, 0);
                            app.push(Route::LookDetail(open_id));
                        }
                    },
                );
            });
        });
    }

    /// Pick a LUT or preset file and make a new Look from it.
    pub fn begin_import(self) {
        self.set_sheet(None);
        self.pick_import(None);
    }

    /// Pick a file that replaces an imported Look, keeping its ID and name.
    pub fn begin_replace(self, id: String) {
        let Some(look) = self.look(&id) else {
            return;
        };
        self.pick_import(Some((look.id, look.name, look.content_hash)));
    }

    fn pick_import(self, replacing: Option<(String, String, String)>) {
        self.root.run(|| {
            spawn(async move {
                let files = pick_files(FilePickRequest::new(false, 1)).await;
                let Some(file) = files.into_iter().next() else {
                    return;
                };
                let bytes = file.data.bytes().to_vec();
                self.start_import(file.name, bytes, replacing);
            });
        });
    }

    /// Convert `bytes` (a LUT or preset named `file_name`) on a worker thread.
    pub fn start_import(
        self,
        file_name: String,
        bytes: Vec<u8>,
        replacing: Option<(String, String, String)>,
    ) {
        let (id, name, replacing) = match replacing {
            Some((id, name, hash)) => (id.clone(), name, Some((id, hash))),
            None => {
                let name = name_from_file(&file_name);
                (import_id(&name, &self.taken_ids()), name, None)
            }
        };
        self.looks.draft.set(Some(ImportDraft {
            file_name: file_name.clone(),
            id: id.clone(),
            name: name.clone(),
            replacing,
            working: true,
            ready: false,
            sending: false,
            error: None,
        }));
        self.looks.draft_grf.with(|g| *g.borrow_mut() = None);
        self.looks.draft_picture.set(None);
        if self.tab() != Tab::Looks {
            self.set_tab(Tab::Looks);
        }
        if self.route_at(Tab::Looks, 0) != Some(Route::LookImport) {
            self.pop_to(Tab::Looks, 0);
            self.push(Route::LookImport);
        }
        self.root.run(|| {
            spawn(async move {
                if self.looks.base_gamma.with(|b| b.borrow().is_none()) {
                    self.ensure_catalog().await;
                }
                let Some(base_gamma) = self.looks.base_gamma.with(|b| b.borrow().clone()) else {
                    self.fail_draft(LookError::ImportFailed(tr("looks.catalog_offline", &[])));
                    return;
                };
                let result = off_thread(move || {
                    let grf = look_media::import(&bytes, &file_name, &id, &name, &base_gamma)?;
                    let picture = picture_for(&grf, Some(&base_gamma));
                    anyhow::Ok((grf, picture))
                })
                .await;
                match result {
                    Some(Ok((grf, picture))) => {
                        self.looks.draft_grf.with(|g| *g.borrow_mut() = Some(grf));
                        self.looks.draft_picture.set(picture);
                        self.looks.draft.update(|draft| {
                            if let Some(draft) = draft {
                                draft.working = false;
                                draft.ready = true;
                            }
                        });
                    }
                    Some(Err(error)) => {
                        log::warn!("Import failed: {error:#}");
                        self.fail_draft(LookError::ImportFailed(error.to_string()));
                    }
                    None => self.fail_draft(LookError::ImportFailed(String::new())),
                }
            });
        });
    }

    fn fail_draft(self, error: LookError) {
        self.looks.draft.update(|draft| {
            if let Some(draft) = draft {
                draft.working = false;
                draft.error = Some(error);
            }
        });
    }

    pub fn set_import_name(self, name: String) {
        self.looks.draft.update(|draft| {
            if let Some(draft) = draft {
                draft.name = name;
            }
        });
    }

    /// Send the converted Look to the camera.
    pub fn send_import(self) {
        let Some(draft) = self.looks.draft.get() else {
            return;
        };
        let Some(grf) = self.looks.draft_grf.with(|g| g.borrow().clone()) else {
            return;
        };
        let name = draft.name.trim().to_owned();
        if !valid_name(&name) {
            self.alert_look_error(LookError::InvalidName);
            return;
        }
        if draft.replacing.is_none()
            && self
                .looks
                .snapshot
                .get()
                .is_some_and(|snapshot| !snapshot.has_room())
        {
            self.alert_look_error(LookError::NoRoom);
            return;
        }
        let grf = match look_media::label(&grf) {
            Ok((_, baked, _)) if baked != name => match look_media::rename(&grf, &name) {
                Ok(renamed) => renamed,
                Err(_) => {
                    self.alert_look_error(LookError::InvalidName);
                    return;
                }
            },
            _ => grf,
        };
        self.looks.draft.update(|d| {
            if let Some(d) = d {
                d.sending = true;
            }
        });
        let demo_grf = grf.clone();
        let id = draft.id.clone();
        let replacing = draft.replacing.as_ref().map(|(_, hash)| hash.clone());
        self.change(
            draft.id.clone(),
            move |fake| fake.put(&demo_grf, false),
            move |context| async move {
                remote::put_content(&context, &id, grf, replacing.as_deref()).await
            },
            |app, ok| {
                if ok {
                    app.discard_import();
                    app.looks.view.set(LooksView::OnCamera);
                    app.pop_to(Tab::Looks, 0);
                } else {
                    // A refused send lets the user try again.
                    app.looks.draft.update(|d| {
                        if let Some(d) = d {
                            d.sending = false;
                        }
                    });
                }
            },
        );
    }

    pub fn discard_import(self) {
        self.looks.draft.set(None);
        self.looks.draft_picture.set(None);
        self.looks.draft_grf.with(|g| *g.borrow_mut() = None);
    }

    /// Demo screens: replace the demo camera's state.
    pub(crate) fn with_fake_looks(self, edit: impl FnOnce(&mut FakeLooks)) {
        self.looks.fake.with(|fake| {
            if let Some(fake) = fake.borrow_mut().as_mut() {
                edit(fake);
            }
        });
    }

    /// Demo screens: mark a catalog Look as being added.
    pub(crate) fn seed_adding(self, id: &str) {
        self.looks.adding.update(|a| {
            a.insert(id.to_owned());
        });
    }

    /// Demo screens: an import ready to send, made from a bundled Look.
    pub(crate) fn seed_import_draft(self) {
        let base = self.looks.base_gamma.with(|b| b.borrow().clone());
        let grf = self.looks.fake.with(|f| {
            f.borrow()
                .as_ref()
                .and_then(|f| f.content(crate::domain::fake_looks::DEMO_CUSTOM_ID))
        });
        let Some(grf) = grf else { return };
        let picture = picture_for(&grf, base.as_deref());
        self.looks.draft_grf.with(|g| *g.borrow_mut() = Some(grf));
        self.looks.draft_picture.set(picture);
        self.looks.draft.set(Some(ImportDraft {
            file_name: "Kodak Gold 200.cube".into(),
            id: "lut-kodak-gold-200".into(),
            name: "Kodak Gold 200".into(),
            replacing: None,
            working: false,
            ready: true,
            sending: false,
            error: None,
        }));
    }

    /// Clear Looks state when the camera changes or disconnects.
    pub(crate) fn reset_looks(self) {
        let store = self.looks_store();
        store.snapshot.set(None);
        store.error.set(None);
        store.blocked.set(None);
        store.busy.set(None);
    }
}
