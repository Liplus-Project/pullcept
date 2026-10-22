//! Process and reservation ownership shared by the Tauri adapter and tests.
use parking_lot::Mutex;
use std::{collections::{HashMap,HashSet}, sync::Arc};

struct Entry<T> { identity: String, config: String, owner: Arc<T> }
struct State<T> { entries: HashMap<String,Entry<T>>, closed: bool }
pub struct Pool<T> { state: Mutex<State<T>> }
impl<T> Default for Pool<T> { fn default() -> Self { Self { state: Mutex::new(State { entries: HashMap::new(), closed:false }) } } }
impl<T> Pool<T> {
    pub fn acquire(&self, home: &str, identity: &str, config: &str, create: impl FnOnce() -> Result<T,String>) -> Result<Arc<T>,String> {
        let mut state = self.state.lock();
        if state.closed { return Err("Pullcept は終了処理中です。".into()) }
        if let Some(entry) = state.entries.get(home) {
            if entry.identity != identity { return Err("同じ認証 HOME のログインが変わりました。既存席を保護するため、Pullcept を再起動してから接続してください。".into()) }
            if entry.config != config { return Err("共通 Codex 接続と process 設定・環境変数が一致しません。既存席を保護するため起動しません。設定を揃えるか Pullcept を再起動してください。".into()) }
            return Ok(entry.owner.clone())
        }
        let owner = Arc::new(create()?);
        state.entries.insert(home.into(),Entry { identity:identity.into(),config:config.into(),owner:owner.clone() });
        Ok(owner)
    }
    /// The app owns teardown. Draining twice cannot stop an owner twice.
    pub fn close(&self) -> Vec<Arc<T>> {
        let mut state = self.state.lock(); state.closed = true;
        state.entries.drain().map(|(_,e)| e.owner).collect()
    }
}

#[derive(Default)] pub struct Threads(Mutex<HashSet<String>>);
pub struct ThreadLease { registry: Arc<Threads>, id: String }
impl ThreadLease {
    pub fn claim(registry: &Arc<Threads>, id: &str) -> Result<Self,String> {
        if !registry.0.lock().insert(id.into()) { return Err("この会話は別の席で利用中です。".into()) }
        Ok(Self { registry:registry.clone(),id:id.into() })
    }
}
impl Drop for ThreadLease { fn drop(&mut self) { self.registry.0.lock().remove(&self.id); } }

#[cfg(test)] mod tests {
    use super::*;
    use std::{sync::atomic::{AtomicUsize,Ordering},thread};
    #[test] fn first_start_race_last_lease_and_app_shutdown_have_one_owner() {
        let pool = Arc::new(Pool::default()); let creates = Arc::new(AtomicUsize::new(0));
        let workers: Vec<_> = (0..8).map(|_| { let p=pool.clone(); let n=creates.clone(); thread::spawn(move || p.acquire("home","account","process",||{n.fetch_add(1,Ordering::SeqCst);Ok(7)}).unwrap()) }).collect();
        let owners: Vec<_> = workers.into_iter().map(|w|w.join().unwrap()).collect();
        assert!(owners.iter().all(|o|Arc::ptr_eq(o,&owners[0]))); assert_eq!(creates.load(Ordering::SeqCst),1);
        let weak=Arc::downgrade(&owners[0]);drop(owners); assert!(weak.upgrade().is_some());
        let drained=pool.close(); assert_eq!(drained.len(),1); assert!(pool.close().is_empty()); drop(drained); assert!(weak.upgrade().is_none());
        assert!(pool.acquire("home","account","process",||Ok(8)).is_err());
    }
    #[test] fn login_and_configuration_mismatch_do_not_replace_existing_owner() {
        let pool=Pool::default(); let a=pool.acquire("home-A","account-A","p",||Ok(1)).unwrap();
        assert!(Arc::ptr_eq(&a,&pool.acquire("home-A","account-A","p",||panic!()).unwrap()));
        assert!(pool.acquire("home-A","account-B","p",||Ok(2)).is_err());
        assert!(pool.acquire("home-A","account-A","changed",||Ok(2)).is_err());
        let b=pool.acquire("home-B","account-B","p",||Ok(2)).unwrap();assert!(!Arc::ptr_eq(&a,&b));
        assert_eq!(*a,1);assert_eq!(pool.close().len(),2);
    }
    #[test] fn failed_start_can_retry_and_failed_claim_cannot_release_another_seat() {
        let pool=Pool::default();assert!(pool.acquire("h","a","p",||Err::<i32,_>("fixture failure".into())).is_err());
        assert_eq!(*pool.acquire("h","a","p",||Ok(1)).unwrap(),1);
        let registry=Arc::new(Threads::default());let lease=ThreadLease::claim(&registry,"A").unwrap();
        assert!(ThreadLease::claim(&registry,"A").is_err());assert!(ThreadLease::claim(&registry,"A").is_err());
        drop(lease);assert!(ThreadLease::claim(&registry,"A").is_ok());
    }
}
