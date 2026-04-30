
use tokio::task::JoinHandle;

use crate::constants::GLOBAL_TASK_HANDLES;

pub async fn register_task(handle: JoinHandle<()>) {
    let mut handles = GLOBAL_TASK_HANDLES.lock().await;
    handles.push(handle);
    tracing::info!("Task registered. Total tasks: {}", handles.len());
}

pub async fn register_task_named(name: &'static str, handle: JoinHandle<()>) {
    let mut handles = GLOBAL_TASK_HANDLES.lock().await;
    tracing::info!("Registering task: {}", name);
    handles.push(handle);
}

pub async fn shutdown_all_tasks() {
    let mut handles = GLOBAL_TASK_HANDLES.lock().await;
    tracing::info!("Shutting down {} tasks", handles.len());
    
    for (i, handle) in handles.drain(..).enumerate() {
        tracing::debug!("Waiting for task {}...", i);
        if let Err(e) = handle.await {
            tracing::error!("Task {} failed to shutdown cleanly: {:?}", i, e);
        }
    }
    
    tracing::info!("All tasks shut down");
}

pub async fn active_task_count() -> usize {
    GLOBAL_TASK_HANDLES.lock().await.len()
}

// For fire-and-forget tasks
pub async fn spawn_and_register<F>(future: F) 
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = tokio::spawn(future);
    register_task(handle).await;
}

pub async fn spawn_named_and_register<F>(name: &'static str, future: F) 
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let task = async move {
        tracing::info!("Task {} starting", name);
        future.await;
        tracing::info!("Task {} completed", name);
    };
    
    let handle = tokio::spawn(task);
    register_task_named(name, handle).await;
}