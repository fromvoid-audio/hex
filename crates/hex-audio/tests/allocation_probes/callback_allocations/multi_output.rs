use allocation_counter::measure;
use hex_engine::multi_output_routing::{
    MultiOutputDestination, MultiOutputError, MultiOutputRouteTable, MultiOutputRouter,
};

#[test]
fn prepared_multi_output_acceptance_and_refusal_leave_heap_untouched() {
    let destination = MultiOutputDestination::Track(7);
    let routes = MultiOutputRouteTable::default()
        .with_route(19, 1, Some(destination), true)
        .unwrap();
    let mut router = MultiOutputRouter::prepare(19, &routes, 1).unwrap();
    let mut main = [[0.25; 2]; 2];
    let mut aux = [[0.5; 2]; 2];
    let mut sink = None;
    let mut refusal = Ok(());
    let mut retry = Ok(());
    let allocations = measure(|| {
        let mut receive = |target, pcm: &[hex_engine::Frame]| sink = Some((target, pcm[1]));
        refusal = router.route_block(&mut [&mut main], 2, &mut receive);
        retry = router.route_block(&mut [&mut main, &mut aux], 2, receive);
    });
    assert_eq!(refusal, Err(MultiOutputError::MissingActiveBus));
    assert_eq!(retry, Ok(()));
    assert_eq!(sink, Some((destination, [0.5; 2])));
    assert_eq!((allocations.count_total, allocations.bytes_total), (0, 0));
    assert_eq!(
        (allocations.count_current, allocations.bytes_current),
        (0, 0)
    );
}
