//! 音频缓冲抽象层与 lock-free 实现。
//!
//! 第二阶段引入 lock-free 环形缓冲（基于 `ringbuf` crate），
//! 替代现有的 `Arc<Mutex<Vec<f32>>>` 设计。
//!
//! ## 架构
//!
//! ```text
//! 麦克风音频回调 ──push_slice──▶ Ring Buffer ──pop_slice──▶ ASR 处理线程
//!         (Producer)                        (Consumer)
//! ```
//!
//! 生产者（音频回调线程）和消费者（ASR 处理线程）分离，
//! 无锁竞争，保证音频线程快速返回。
//!
//! ## 当前状态
//!
//! 本模块提供 lock-free 缓冲工具，但尚未替换现有调用点
//!（`recorder.rs`、`streaming/audio.rs`、`streaming/session.rs`）。
//! 后续迁移步骤将逐步切换到 `create_ring_buffer` 返回的分离句柄。

use ringbuf::traits::{Consumer, Observer, Producer, Split};

/// 保留原 AudioBuffer trait（用于简单场景或需要单句柄的同步访问）。
///
/// 对于实时音频场景，请使用 [`create_ring_buffer`] 返回的分离句柄。
pub trait AudioBuffer: Send + Sync {
    /// 生产者写入音频样本（由音频回调线程调用，必须快速返回）。
    fn push(&self, samples: &[f32]);

    /// 消费者取出全部累积样本（由 ASR 处理线程调用）。
    fn drain(&self) -> Vec<f32>;

    /// 当前缓冲中的样本数。
    fn len(&self) -> usize;

    /// 缓冲是否为空。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 清空缓冲。
    fn clear(&self);
}

/// 创建一对 lock-free 环形缓冲句柄。
///
/// 返回 `(生产者, 消费者)`：
/// - 生产者由音频采集线程持有，调用 [`push_producer`] 写入样本
/// - 消费者由 ASR 处理线程持有，调用 [`drain_consumer`] 取出样本
///
/// 生产者和消费者各自独立，无锁竞争。缓冲满时 `push_slice` 丢弃溢出部分，
/// 保证音频回调线程永远快速返回。
///
/// # 容量建议
///
/// 16kHz 单声道：1 秒 = 16000 样本。建议容量 = 期望最大延迟 × 采样率。
/// 例如 2 秒延迟缓冲 → 容量 32000。
pub fn create_ring_buffer(
    capacity: usize,
) -> (
    impl Producer<Item = f32> + Send,
    impl Consumer<Item = f32> + Send,
) {
    let rb = ringbuf::HeapRb::<f32>::new(capacity);
    rb.split()
}

/// 向生产者写入样本，返回实际写入数量。
///
/// 缓冲满时丢弃溢出部分（不阻塞），保证音频回调线程快速返回。
/// 音频回调中应直接调用此函数，不等待。
pub fn push_producer<P: Producer<Item = f32>>(prod: &mut P, samples: &[f32]) -> usize {
    prod.push_slice(samples)
}

/// 从消费者中取出全部累积样本，清空缓冲。
///
/// ASR 处理线程定期调用此函数取走音频数据。
pub fn drain_consumer<C: Consumer<Item = f32>>(cons: &mut C) -> Vec<f32> {
    let mut out = Vec::new();
    let mut buf = [0.0f32; 4096];
    loop {
        let n = cons.pop_slice(&mut buf);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    out
}

/// 消费者当前缓冲中的样本数。
pub fn consumer_len<C: Observer<Item = f32>>(cons: &C) -> usize {
    cons.occupied_len()
}

/// 消费者是否为空。
pub fn consumer_is_empty<C: Observer<Item = f32>>(cons: &C) -> bool {
    cons.is_empty()
}

/// 生产者剩余可写容量。
pub fn producer_vacant<P: Observer<Item = f32>>(prod: &P) -> usize {
    prod.vacant_len()
}

/// 生产者是否已满。
pub fn producer_is_full<P: Observer<Item = f32>>(prod: &P) -> bool {
    prod.is_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn push_and_drain_basic() {
        let (mut prod, mut cons) = create_ring_buffer(1024);
        let samples = vec![1.0_f32, 2.0, 3.0];
        let n = push_producer(&mut prod, &samples);
        assert_eq!(n, 3);
        assert_eq!(consumer_len(&cons), 3);
        let out = drain_consumer(&mut cons);
        assert_eq!(out, samples);
        assert!(consumer_is_empty(&cons));
    }

    #[test]
    fn buffer_full_drops_overflow() {
        let (mut prod, mut cons) = create_ring_buffer(4);
        let n1 = push_producer(&mut prod, &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(n1, 4);
        assert!(producer_is_full(&prod));
        // 溢出应被丢弃，不阻塞
        let n2 = push_producer(&mut prod, &[5.0, 6.0]);
        assert_eq!(n2, 0);
        let out = drain_consumer(&mut cons);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn multiple_pushes_accumulate() {
        let (mut prod, mut cons) = create_ring_buffer(1024);
        push_producer(&mut prod, &[1.0, 2.0]);
        push_producer(&mut prod, &[3.0, 4.0, 5.0]);
        assert_eq!(consumer_len(&cons), 5);
        let out = drain_consumer(&mut cons);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn drain_empty_returns_empty_vec() {
        let (_prod, mut cons) = create_ring_buffer(1024);
        let out = drain_consumer(&mut cons);
        assert!(out.is_empty());
    }

    #[test]
    fn partial_push_when_near_full() {
        let (mut prod, mut cons) = create_ring_buffer(4);
        push_producer(&mut prod, &[1.0, 2.0, 3.0]);
        // 只剩 1 个空位，写入 3 个应只写入 1 个
        let n = push_producer(&mut prod, &[4.0, 5.0, 6.0]);
        assert_eq!(n, 1);
        assert!(cons.is_full());
        let out = drain_consumer(&mut cons);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn cross_thread_producer_consumer() {
        // 验证 lock-free 缓冲在生产者线程、消费者主线程间正确传递数据
        let (mut prod, mut cons) = create_ring_buffer(2048);
        let handle = thread::spawn(move || {
            for i in 0..1000u32 {
                push_producer(&mut prod, &[i as f32]);
            }
            prod
        });
        handle.join().unwrap();

        let mut out = Vec::new();
        loop {
            let chunk = drain_consumer(&mut cons);
            if chunk.is_empty() {
                break;
            }
            out.extend(chunk);
        }
        assert_eq!(out.len(), 1000);
        // 验证 FIFO 顺序
        for (i, &v) in out.iter().enumerate() {
            assert_eq!(v, i as f32);
        }
    }

    #[test]
    fn producer_vacant_decreases_after_push() {
        let (mut prod, _cons) = create_ring_buffer(8);
        let initial = producer_vacant(&prod);
        assert_eq!(initial, 8);
        push_producer(&mut prod, &[1.0, 2.0, 3.0]);
        assert_eq!(producer_vacant(&prod), 5);
    }
}
