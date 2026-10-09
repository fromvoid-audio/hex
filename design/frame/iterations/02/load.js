(async () => {
  const font = new FontFace('Ioskeley Mono', 'url(/assets/fonts/ioskeley-mono/ioskeley-mono-regular.ttf)');
  document.fonts.add(await font.load());
  Frame.registerStyleguide(await (await fetch('/design/frame/live-layout-styleguide.json')).json());
  const design = await (await fetch('/design/frame/hex-live-r67.json')).json();
  Frame.registerDocument(design);
  Frame.agent.selectDesign(design.id);
  await Frame.agent.renderReady();
  Frame.view.fit();
  Frame.view.setMode('interact');
  const saveExport = async (name, body) => {
    const response = await fetch('/export/' + name, {method: 'POST', body});
    if (!response.ok) throw new Error('Frame export failed: ' + name + ' (' + response.status + ')');
  };
  const themeBoard = await (await fetch('/design/frame/theme-palette.frame.json')).json();
  Frame.registerDocument(themeBoard);
  Frame.agent.selectDesign(themeBoard.id);
  await Frame.agent.renderReady();
  await saveExport('theme-palette.png', (await Frame.agent.exportImage({designId: themeBoard.id, scale: 1})).blob);
  Frame.agent.selectDesign(design.id);
  // Frame's own authoring API exports the portable handoff and both views.
  const reviews = {theme: Frame.review.check(themeBoard.id)};
  for (const state of ['default', 'playing', 'session', 'session-playing', 'clip-colors']) {
    Frame.agent.setState(design.id, state);
    await Frame.agent.renderReady();
    reviews[state] = Frame.review.check(design.id);
    const stateExport = await Frame.agent.exportImage({designId: design.id, scale: 1});
    await saveExport('state-' + state + '.png', stateExport.blob);
    if (state === 'playing' || state === 'session-playing' || state === 'clip-colors') {
      const exported = await Frame.agent.exportImage({designId: design.id, scale: 1});
      await saveExport(state === 'playing' ? 'arrangement.png' : state === 'session-playing' ? 'session.png' : 'palette.png', exported.blob);
    }
  }
  Frame.agent.setState(design.id, 'default');
  reviews.interactions = await Frame.review.run(design.id, [
    {name: 'play', actions: [{type:'click',elementId:'play'}], expectedState:'playing'},
    {name: 'pause', state:'playing', actions:[{type:'click',elementId:'pause'}], expectedState:'default'},
    {name: 'session view', actions:[{type:'click',elementId:'session-tab'}], expectedState:'session'},
    {name: 'scene launch', state:'session', actions:[{type:'click',elementId:'scene-0'}], expectedState:'session-playing'},
    {name: 'session stop', state:'session-playing', actions:[{type:'click',elementId:'session-stop'}], expectedState:'session'},
    {name: 'open colors', state:'session', actions:[{type:'click',elementId:'color-menu'}], expectedState:'clip-colors'},
    {name: 'close colors', state:'clip-colors', actions:[{type:'click',elementId:'color-close'}], expectedState:'session'},
    {name: 'arrangement return', state:'session', actions:[{type:'click',elementId:'arrange-tab'}], expectedState:'default'}
  ]);
  await saveExport('review.json', JSON.stringify(reviews, null, 2));
  await saveExport('hex.frame.json', JSON.stringify(Frame.project.export(design.id)));
  window.hexDesignReady = true;
})().catch(error => { console.error(error); document.title = 'hex design failed'; });
