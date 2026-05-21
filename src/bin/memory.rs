#[cfg(not(feature = "jemalloc"))]
use std::alloc::System;
use std::alloc::{GlobalAlloc, Layout};
use std::env;
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

use str0m_benchmarks::{
    EnqueueSharedHarness, EnqueueVecHarness, FullEgressSharedHarness, FullEgressVecHarness,
    FullRelaySharedHarness, FullRelayVecHarness, ReceiveMediaSharedHarness, ReceiveMediaVecHarness,
    ReceiveRtpSharedHarness, ReceiveRtpVecHarness, benchmark_fanouts, benchmark_targets,
    forward_shared, forward_vec, make_payload, packet_template_shared, packet_template_vec,
    shared_payload,
};

const PAYLOAD_SCENARIOS: &[PayloadScenario] = &[
    PayloadScenario {
        label: "audio-160B",
        size: 160,
    },
    PayloadScenario {
        label: "video-1350B",
        size: 1350,
    },
];
const ENQUEUE_ROUNDS: usize = 64;
const FULL_EGRESS_ROUNDS: usize = 64;
const DEFAULT_FULL_RELAY_ROUNDS: usize = 64;
const RECEIVE_ROUNDS: usize = 64;

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

#[cfg(feature = "jemalloc")]
static INNER_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
#[cfg(not(feature = "jemalloc"))]
static INNER_ALLOCATOR: System = System;

static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

struct CountingAllocator;

struct PayloadScenario {
    label: &'static str,
    size: usize,
}

#[derive(Clone, Copy)]
struct AllocationStats {
    alloc_calls: u64,
    dealloc_calls: u64,
    realloc_calls: u64,
    allocated_bytes: u64,
    deallocated_bytes: u64,
}

// SAFETY: this allocator delegates every request to the selected allocator with
// the original pointer and layout. The counters are side effects that do not
// affect ownership.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: the request is forwarded unchanged to the selected allocator.
        unsafe { INNER_ALLOCATOR.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: the pointer and layout come from the allocator contract.
        unsafe { INNER_ALLOCATOR.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        REALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        // SAFETY: the pointer, layout and new size are forwarded unchanged.
        unsafe { INNER_ALLOCATOR.realloc(ptr, layout, new_size) }
    }
}

fn main() {
    let fanouts = benchmark_fanouts();
    let full_relay_rounds = full_relay_rounds();

    println!(
        "| group | scenario | variant | packets | alloc calls | dealloc calls | realloc calls | allocated bytes | retained bytes | bytes/packet |"
    );
    println!("|---|---|---|---:|---:|---:|---:|---:|---:|---:|");

    for scenario in PAYLOAD_SCENARIOS {
        for &fanout in &fanouts {
            print_packet_fanout(scenario, fanout);
            print_enqueue(scenario, fanout);
            print_full_egress(scenario, fanout);
            print_full_relay_rtp(scenario, fanout, full_relay_rounds);
            print_receive_rtp_fanout(scenario, fanout);
            print_receive_media_fanout(scenario, fanout);
        }
        print_receive_rtp_event(scenario);
        print_receive_media_event(scenario);
    }
}

fn full_relay_rounds() -> usize {
    env::var("FULL_RELAY_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|rounds| *rounds > 0)
        .unwrap_or(DEFAULT_FULL_RELAY_ROUNDS)
}

fn print_packet_fanout(scenario: &PayloadScenario, fanout: usize) {
    let targets = benchmark_targets(fanout);
    let mut vec_out = Vec::with_capacity(fanout);
    let vec_template = packet_template_vec(scenario.size);
    let stats = measure_allocations(|| {
        forward_vec(black_box(&vec_template), black_box(&targets), &mut vec_out);
        black_box(&vec_out);
    });
    print_row("packet_fanout", scenario, fanout, "base_vec", fanout, stats);

    let mut shared_out = Vec::with_capacity(fanout);
    let shared_template = packet_template_shared(scenario.size);
    let stats = measure_allocations(|| {
        forward_shared(
            black_box(&shared_template),
            black_box(&targets),
            &mut shared_out,
        );
        black_box(&shared_out);
    });
    print_row("packet_fanout", scenario, fanout, "arc_meta", fanout, stats);
}

fn print_enqueue(scenario: &PayloadScenario, fanout: usize) {
    let payload_vec = make_payload(scenario.size);
    let mut harness = EnqueueVecHarness::new(fanout);
    let stats = measure_allocations(|| {
        harness.enqueue_vec(black_box(&payload_vec), ENQUEUE_ROUNDS);
        black_box(&harness);
    });
    print_row(
        "enqueue",
        scenario,
        fanout,
        "base_vec",
        fanout * ENQUEUE_ROUNDS,
        stats,
    );

    let payload_shared = shared_payload(scenario.size);
    let mut harness = EnqueueSharedHarness::new(fanout);
    let stats = measure_allocations(|| {
        harness.enqueue_shared(black_box(&payload_shared), ENQUEUE_ROUNDS);
        black_box(&harness);
    });
    print_row(
        "enqueue",
        scenario,
        fanout,
        "arc_meta",
        fanout * ENQUEUE_ROUNDS,
        stats,
    );
}

fn print_full_egress(scenario: &PayloadScenario, fanout: usize) {
    let payload_vec = make_payload(scenario.size);
    let mut harness = FullEgressVecHarness::new(fanout);
    let stats = measure_allocations(|| {
        let transmit_count = harness.egress_vec(black_box(&payload_vec), FULL_EGRESS_ROUNDS);
        black_box(transmit_count);
    });
    print_row(
        "full_egress",
        scenario,
        fanout,
        "base_vec",
        fanout * FULL_EGRESS_ROUNDS,
        stats,
    );

    let payload_shared = shared_payload(scenario.size);
    let mut harness = FullEgressSharedHarness::new(fanout);
    let stats = measure_allocations(|| {
        let transmit_count = harness.egress_shared(black_box(&payload_shared), FULL_EGRESS_ROUNDS);
        black_box(transmit_count);
    });
    print_row(
        "full_egress",
        scenario,
        fanout,
        "arc_meta",
        fanout * FULL_EGRESS_ROUNDS,
        stats,
    );
}

fn print_full_relay_rtp(scenario: &PayloadScenario, fanout: usize, full_relay_rounds: usize) {
    let mut harness = FullRelayVecHarness::new(fanout, scenario.size, full_relay_rounds);
    let stats = measure_allocations(|| {
        let transmit_count = harness.relay_vec();
        black_box(transmit_count);
    });
    print_row(
        "full_relay_rtp",
        scenario,
        fanout,
        "base_vec",
        fanout * full_relay_rounds,
        stats,
    );

    let mut harness = FullRelaySharedHarness::new(fanout, scenario.size, full_relay_rounds);
    let stats = measure_allocations(|| {
        let transmit_count = harness.relay_shared();
        black_box(transmit_count);
    });
    print_row(
        "full_relay_rtp",
        scenario,
        fanout,
        "arc_meta",
        fanout * full_relay_rounds,
        stats,
    );
}

fn print_receive_rtp_event(scenario: &PayloadScenario) {
    let mut harness = ReceiveRtpVecHarness::new(scenario.size, RECEIVE_ROUNDS);
    let stats = measure_allocations(|| {
        let payload_bytes = harness.receive_events();
        black_box(payload_bytes);
    });
    print_row(
        "receive_rtp_event",
        scenario,
        1,
        "base_vec",
        RECEIVE_ROUNDS,
        stats,
    );
    let mut harness = ReceiveRtpSharedHarness::new(scenario.size, RECEIVE_ROUNDS);
    let stats = measure_allocations(|| {
        let payload_bytes = harness.receive_events();
        black_box(payload_bytes);
    });
    print_row(
        "receive_rtp_event",
        scenario,
        1,
        "arc_meta",
        RECEIVE_ROUNDS,
        stats,
    );
}

fn print_receive_rtp_fanout(scenario: &PayloadScenario, fanout: usize) {
    let targets = benchmark_targets(fanout);
    let mut vec_out = Vec::with_capacity(fanout);
    let mut harness = ReceiveRtpVecHarness::new(scenario.size, RECEIVE_ROUNDS);
    let stats = measure_allocations(|| {
        let forwarded = harness.fanout_vec(black_box(&targets), &mut vec_out);
        black_box(forwarded);
        black_box(&vec_out);
    });
    print_row(
        "receive_rtp_fanout",
        scenario,
        fanout,
        "base_vec",
        fanout * RECEIVE_ROUNDS,
        stats,
    );

    let mut shared_out = Vec::with_capacity(fanout);
    let mut harness = ReceiveRtpSharedHarness::new(scenario.size, RECEIVE_ROUNDS);
    let stats = measure_allocations(|| {
        let forwarded = harness.fanout_shared(black_box(&targets), &mut shared_out);
        black_box(forwarded);
        black_box(&shared_out);
    });
    print_row(
        "receive_rtp_fanout",
        scenario,
        fanout,
        "arc_meta",
        fanout * RECEIVE_ROUNDS,
        stats,
    );
}

fn print_receive_media_event(scenario: &PayloadScenario) {
    let mut harness = ReceiveMediaVecHarness::new(scenario.size, RECEIVE_ROUNDS);
    let stats = measure_allocations(|| {
        let payload_bytes = harness.receive_events();
        black_box(payload_bytes);
    });
    print_row(
        "receive_media_event",
        scenario,
        1,
        "base_vec",
        RECEIVE_ROUNDS,
        stats,
    );
    let mut harness = ReceiveMediaSharedHarness::new(scenario.size, RECEIVE_ROUNDS);
    let stats = measure_allocations(|| {
        let payload_bytes = harness.receive_events();
        black_box(payload_bytes);
    });
    print_row(
        "receive_media_event",
        scenario,
        1,
        "arc_meta",
        RECEIVE_ROUNDS,
        stats,
    );
}

fn print_receive_media_fanout(scenario: &PayloadScenario, fanout: usize) {
    let targets = benchmark_targets(fanout);
    let mut vec_out = Vec::with_capacity(fanout);
    let mut harness = ReceiveMediaVecHarness::new(scenario.size, RECEIVE_ROUNDS);
    let stats = measure_allocations(|| {
        let forwarded = harness.fanout_vec(black_box(&targets), &mut vec_out);
        black_box(forwarded);
        black_box(&vec_out);
    });
    print_row(
        "receive_media_fanout",
        scenario,
        fanout,
        "base_vec",
        fanout * RECEIVE_ROUNDS,
        stats,
    );

    let mut shared_out = Vec::with_capacity(fanout);
    let mut harness = ReceiveMediaSharedHarness::new(scenario.size, RECEIVE_ROUNDS);
    let stats = measure_allocations(|| {
        let forwarded = harness.fanout_shared(black_box(&targets), &mut shared_out);
        black_box(forwarded);
        black_box(&shared_out);
    });
    print_row(
        "receive_media_fanout",
        scenario,
        fanout,
        "arc_meta",
        fanout * RECEIVE_ROUNDS,
        stats,
    );
}

fn measure_allocations(mut run: impl FnMut()) -> AllocationStats {
    reset_counters();
    run();
    AllocationStats {
        alloc_calls: ALLOC_CALLS.load(Ordering::Relaxed),
        dealloc_calls: DEALLOC_CALLS.load(Ordering::Relaxed),
        realloc_calls: REALLOC_CALLS.load(Ordering::Relaxed),
        allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        deallocated_bytes: DEALLOCATED_BYTES.load(Ordering::Relaxed),
    }
}

fn reset_counters() {
    ALLOC_CALLS.store(0, Ordering::Relaxed);
    DEALLOC_CALLS.store(0, Ordering::Relaxed);
    REALLOC_CALLS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    DEALLOCATED_BYTES.store(0, Ordering::Relaxed);
}

fn print_row(
    group: &'static str,
    scenario: &PayloadScenario,
    fanout: usize,
    variant: &'static str,
    packets: usize,
    stats: AllocationStats,
) {
    let retained_bytes = stats.allocated_bytes as i128 - stats.deallocated_bytes as i128;
    let bytes_per_packet = stats.allocated_bytes / packets.max(1) as u64;
    println!(
        "| {group} | {}-{fanout}dst | {variant} | {packets} | {} | {} | {} | {} | {} | {} |",
        scenario.label,
        stats.alloc_calls,
        stats.dealloc_calls,
        stats.realloc_calls,
        stats.allocated_bytes,
        retained_bytes,
        bytes_per_packet,
    );
}
