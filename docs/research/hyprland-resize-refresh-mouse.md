# Hyprland Resize, Refresh and Mouse Study

## Resumo

O root cause mais provável para Zen/Firefox/Gecko não parecer redimensionar em tempo real no backend nativo do Oblivion é uma combinação de protocolo e frame loop:

- O resize interativo em Oblivion apenas enfileira `xdg_toplevel.configure` e não altera `render_generation` até o cliente commitar um novo buffer. Isso está explicitamente coberto por `src/compositor/tests/windows.rs:288-302`.
- No backend nativo, o configure pendente só é flushado por `server.present_frame()`, e esse método é chamado depois de `paint_server_frame()` e `scanout.present()` no loop principal (`src/native_output.rs:367-380`). O nested output, em contraste, chama `present_frame()` no fim de todo draw (`src/nested_output.rs:230-247`).
- O caminho nativo redesenha e copia o frame inteiro em cada movimento de mouse (`src/native_output.rs:992-1009`, `src/native_output.rs:2801-2824`), porque cursor faz parte do `DesktopVisualState` (`src/compositor/render.rs:22-36`) e não há cursor plane nem damage de cursor.
- O ACK de configure já existe no Oblivion, mas é mais estrito que Hyprland: Oblivion promove apenas serial exato (`src/compositor/mod.rs:2289-2297`), enquanto Hyprland promove o resize mais recente com serial menor ou igual ao ACK (`WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp:1414-1428`). Isso é uma diferença real para clientes que coalescem ACKs ou recebem configure sem resize entre configures de tamanho.

Prioridade de correção sugerida:

1. Separar `present_frame()` em fases de prepare/finish para enviar resize configure antes da pintura e entregar callbacks/presentation depois da apresentação.
2. Ajustar `ack_xdg_surface_configure` para promover o maior resize serial `<= ack_serial`, igual ao Hyprland.
3. Adicionar cursor de hardware ou, no mínimo, damage-only para cursor de software no backend nativo.
4. Introduzir damage tracking real no output nativo para evitar compor/copiar 1920x1080 a cada evento de mouse.
5. Fortalecer seleção de modo KMS com lista de candidatos testáveis, seguindo a cascata do Hyprland.

## Arquivos Hyprland estudados

- `WM para Referencia/Hyprland-main/src/config/shared/monitor/Parser.cpp`
  - `parseMode` reconhece `preferred`, `highrr`, `highres`, `maxwidth`, modeline DRM e `WxH@Hz` (`:110-142`).

- `WM para Referencia/Hyprland-main/src/output/Monitor.cpp`
  - `applyMonitorRule` monta uma lista curta de modos candidatos e testa cada modo antes de aceitar (`:696-930`).
  - `highrr` prioriza refresh e depois resolução (`:761-771`).
  - `highres` prioriza resolução e depois refresh (`:772-783`).
  - `maxwidth` prioriza largura e depois refresh (`:784-794`).
  - `WxH@Hz` ordena por proximidade e injeta modo custom se o melhor modo não é próximo o suficiente (`:795-817`).
  - A aplicação real testa `m_state.test()` antes de aceitar um modo (`:855-893`) e ainda tenta fallback custom/qualquer modo (`:896-930`).

- `WM para Referencia/Hyprland-main/src/desktop/view/Window.hpp`
  - Mantém geometria lógica/real e estado de ACK: `m_reportedSize`, `m_pendingReportedSize`, `m_pendingSizeAck`, `m_pendingSizeAcks` (`:146-159`).

- `WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp`
  - `sendWindowSize` evita spam, envia `setSize` para XDG e guarda `(serial, size)` (`:1633-1655`).
  - `onAck` procura o resize pendente mais recente cujo serial é `<= ack_serial`, aplica `ackedSize` na surface pendente e remove serials antigos (`:1414-1428`).
  - `commitWindow` danifica a surface/janela no commit em vez de redesenhar tudo por padrão (`:2575-2633`, estudado).

- `WM para Referencia/Hyprland-main/src/layout/target/WindowTarget.cpp`
  - Atualiza `m_position/m_size` e `m_realPosition/m_realSize`, danifica old/new window com scope guard e chama `sendWindowSize` (`:40-60`, `:87-101`, `:107-120`).

- `WM para Referencia/Hyprland-main/src/render/pass/SurfacePassElement.cpp`
  - Durante resize interativo, surface pequena nao é esticada para preencher o alvo; usa o tamanho corrigido da surface (`:20-50`).
  - `squishOversized` só limita oversized surface ao box da janela (`:68-73`).

- `WM para Referencia/Hyprland-main/src/managers/PointerManager.cpp`
  - `softwareLockedFor` só ativa cursor software quando há lock ou falha de hardware (`:58-99`).
  - `updateCursorBackend` tenta cursor de hardware por monitor e cai para software se necessário (`:284-315`).
  - `onCursorMoved` move cursor de hardware no output e não agenda repaint total quando hardware funciona (`:319-352`).
  - `attemptHardwareCursor` renderiza/aplica buffer de cursor no backend (`:360-397`).
  - `renderSoftwareCursorsFor` só desenha cursor no render pass quando hardware falhou/lockou (`:620-664`).
  - `damageCursor` danifica somente a caixa do cursor (`:1118-1129`).

- `WM para Referencia/Hyprland-main/src/managers/input/InputManager.cpp`
  - Movimento de mouse só agenda frame para cursor quando software cursor está ativo e o monitor não deve pular frame schedule (`:267-276`, `:559-560`).

- `WM para Referencia/Hyprland-main/src/output/Monitor.cpp`
  - O output escuta `frame`, `needsFrame` e `present`, e repassa presentation timing real/fallback ao protocolo (`:99-140`).

- `WM para Referencia/Hyprland-main/src/output/MonitorFrameScheduler.cpp`
  - Novo scheduler usa explicit sync, render no frame event, e quando perde prazo renderiza antes e commita no presented/doLater (`:14-83`, `:86-150`).

- `WM para Referencia/Hyprland-main/src/render/Renderer.cpp`
  - Render só continua se `needsFrame`/force frame existem (`:2070-2071`).
  - Usa damage region e só renderiza workspace se há damage (`:2136-2164`).
  - Renderiza software cursor somente no caminho necessário (`:2207-2212`).
  - Adiciona damage ao output state e commita com modo vsync/immediate (`:2249-2255`).
  - Reagenda frame se damage/debug/VFR exigem (`:2260-2263`).
  - `commitPendingAndDoExplicitSync` commita estado do monitor e faz rollback/damage se falhar (`:2466-2487`).
  - `sendFrameEventsToWorkspace` envia frame callbacks para surfaces visíveis (`:2504-2510`).
  - `damageSurface`, `damageWindow`, `damageBox` marcam regiões específicas e agendam frames conforme necessário (`:2698-2812`).

## Padrões relevantes

### 1. Mode selection com fallback testável

Hyprland não escolhe só um modo e tenta commitar. Ele transforma a preferência em uma lista de candidatos:

- `preferred`: preferred mode e primeiros fallbacks.
- `highrr`: maior refresh, desempate por resolução.
- `highres`: maior resolução, desempate por refresh.
- `maxwidth`: maior largura, desempate por refresh.
- `WxH@Hz`: modos mais próximos, com custom mode se necessário.

Depois testa cada candidato com o backend antes de aceitar (`Monitor.cpp:855-893`). Isso é especialmente importante para `1920x1080@165`, porque o modo “ideal” pode não ser aceito no CRTC/conector/formato atual.

### 2. ACK serial-aware

Hyprland guarda uma fila de `(serial, size)` e no ACK escolhe o último serial `<= ack_serial` (`Window.cpp:1414-1421`). Isso acompanha a semântica do xdg-shell: um ACK de configure mais novo também confirma estados anteriores ainda pendentes.

### 3. Resize sem esticar buffer antigo

Hyprland separa geometria real da janela e geometria/tamanho reportado ao cliente. No render, detecta resize interativo e evita ampliar uma surface pequena para preencher o box novo (`SurfacePassElement.cpp:26-50`). O resultado é: a moldura/geometria pode acompanhar o mouse, mas o buffer antigo do cliente não vira uma imagem borrada/esticada até o cliente commitar o tamanho novo.

### 4. Cursor não deve causar repaint total

Hyprland tenta cursor de hardware. Se hardware cursor funciona, movimento de mouse chama `moveCursor` no output (`PointerManager.cpp:350-351`). Quando cai para software, o dano fica restrito à caixa do cursor (`PointerManager.cpp:1118-1129`), e o input só agenda frame nessa condição (`InputManager.cpp:275-276`, `:559-560`).

### 5. Frame loop orientado por output events

Hyprland renderiza a partir de eventos de output (`frame`, `needsFrame`, `present`) e usa damage para decidir se há trabalho. O scheduler novo integra explicit sync e pode separar render de commit para não perder vblank (`MonitorFrameScheduler.cpp:20-83`).

## Diferenças no Oblivion

### `src/native_output.rs`

- `NativeModePreference` já parseia `auto`, `preferred`, `highres`, `highrr` e `WxH@Hz` (`:70-140`), mas `select_kms_mode` escolhe um único modo (`:2449-2491`). Não há lista de candidatos testados como no Hyprland.
- O loop nativo pinta/apresenta antes de chamar `server.present_frame()` (`:367-380`). Como `present_frame()` é quem flushou `pending_resize_configure`, o configure de resize sai no fim do ciclo, não antes da pintura.
- O sleep é timer-driven: input usa 1 ms; atividade usa intervalo de refresh; idle usa intervalo maior (`:403-443`). Não há scheduler orientado por DRM frame/present event no nível de compositor.
- Movimento de ponteiro sempre chama `effect.request_redraw()` (`:992-1009`, `:1012-1029`).
- O cursor faz parte do frame composto via `NativeInputState::desktop_visual_state` (`:785-787`) e `DesktopSceneRenderer` desenha cursor no buffer final (`src/compositor/render.rs:131-135`, `:612-640`).
- `NativeFrameRenderer` gera o frame completo usando surfaces, shell e cursor (`src/native_output.rs:462-518`).
- O GBM scanout copia o frame inteiro para staging e escreve o BO inteiro (`src/native_output.rs:2801-2824`), depois agenda pageflip se não há flip pendente (`:2840-2869`).

### `src/compositor/mod.rs`

- O estado tem `pending_resize_configure`, `sent_resize_commits` e `pending_resize_commits` (`:155-157`).
- `queue_resize_root_window_to` apenas grava um pending configure; não altera `render_generation` (`:2135-2161`).
- `flush_pending_resize_configure` envia o configure pendente (`:2164-2176`).
- `send_resize_configure_to` envia `xdg_toplevel.configure`, cria serial de `xdg_surface.configure` e guarda o resize por `(surface_id, serial)` (`:2225-2254`).
- `ack_xdg_surface_configure` só promove resize quando o ACK serial bate exatamente com `(surface_id, serial)`; depois remove todos os serials `<= ack_serial` (`:2289-2297`). Se o ACK for de um configure posterior sem resize, o resize anterior é descartado em vez de promovido.
- `take_pending_resize_commit_placement` consome um resize ACKed por surface e ajusta posicionamento para edges top/left usando o tamanho efetivamente commitado (`:2266-2287`).
- `has_pending_frame_work` agora inclui `pending_resize_configure` (`:2612-2616`), o que é bom e está coberto por teste.
- `present_frame()` em `src/compositor/server.rs` faz muita coisa numa fase só: commit de explicit sync pronto, color, flush resize configure, buffer releases, frame callbacks e presentation feedbacks (`src/compositor/server.rs:218-225`). Isso dificulta enviar configure antes do render e callbacks depois da apresentação.

### `src/compositor/render.rs`

- `DesktopSceneRenderer::compose_request` reconstrói a cena por `content_generation`, copia a cena inteira para o frame e desenha shell/cursor por cima (`:108-135`, `:146-190`).
- `draw_client_surfaces_scaled` blita cada surface para `surface.width/height` (`:257-286`).
- Se o buffer size difere do target size, `blit_surface_to_rect` escala o buffer para o target (`:643-723`). Isso é correto para viewport/scale, mas perigoso se o compositor começar a atualizar o target visual durante resize sem separar "box da janela" de "buffer commitado".
- `server_frame_rects_for_surface` retorna vazio (`:333-335`), então ainda não existe uma moldura/área de resize desenhada independentemente do buffer do cliente.

### `src/compositor/output.rs`

- Output expõe uma única mode atual/preferida com refresh normalizado (`:69-102`, `:143-154`).
- O refresh é usado para `wl_output` e presentation refresh nsec (`:81-87`), mas não existe camada de política de modos com candidatos/fallback como Hyprland.

### `src/compositor/explicit_sync.rs`

- Há suporte protocolar para acquire/release syncobj (`:29-67`, `:108-190`).
- O commit explícito só entra quando o acquire point já sinalizou (`src/compositor/mod.rs:990-1027`, `:2686-2697`).
- Não há equivalente do scheduler do Hyprland que usa explicit sync/output state para renderizar cedo e commitar no presented/vblank.

### `src/compositor/tests/windows.rs`

- `resize_drag_coalesces_pointer_updates_until_present_frame` valida que movimentos são coalescidos até `PresentFrame` (`:163-180`).
- `queued_resize_configure_reports_pending_frame_work` valida que resize pendente conta como trabalho de frame (`:182-198`).
- `ack_configure_promotes_matching_resize_commit` cobre apenas serial exato (`:200-224`).
- `resize_configure_without_client_commit_does_not_advance_render_generation` fixa a semântica atual: configure-only resize não muda geração renderizável (`:287-303`).

## Recomendações implementáveis

### 1. Separar `present_frame()` em prepare/finish

Problema: `present_frame()` mistura "enviar configure ao cliente" com "entregar frame callbacks/presentation feedbacks". No native, isso força o resize configure a sair depois da pintura.

Patch sugerido:

```rust
// src/compositor/server.rs
pub fn prepare_frame(&mut self) {
    self.state.commit_ready_explicit_sync_buffers();
    color::flush_pending_color_info(&mut self.state);
    self.state.flush_pending_resize_configure();
    let _ = self.display.flush_clients();
}

pub fn finish_frame(&mut self) {
    self.state.release_pending_buffers();
    self.state.complete_pending_frame_callbacks();
    self.state.complete_pending_presentation_feedbacks();
    let _ = self.display.flush_clients();
}

pub fn present_frame(&mut self) {
    self.prepare_frame();
    self.finish_frame();
}
```

No native:

```rust
// src/native_output.rs
let pending_protocol_work = server.has_pending_frame_work();
if pending_protocol_work {
    server.prepare_frame();
}

let render_generation = server.render_generation();
if accepted > 0
    || render_generation != last_render_generation
    || pending_protocol_work
    || redraw_requested
{
    scanout.paint_server_frame(&mut frame_renderer, &server, &input_state)?;
    scanout.present(kms.file().as_fd(), target.crtc_id)?;
    last_render_generation = render_generation;
    server.finish_frame();
}
```

Risco: callbacks de frame/presentation ainda ficariam ligados ao schedule de pageflip, não necessariamente ao pageflip concluído. O próximo passo correto é chamar `finish_frame()` quando `drain_page_flip_events` confirmar apresentação.

Teste mínimo:

- Adicionar um teste de unidade para o servidor controlado: enfileirar resize, chamar uma nova fase `prepare_frame`, verificar que o cliente recebeu `xdg_toplevel.configure` antes de qualquer commit de buffer.
- Manter `resize_drag_coalesces_pointer_updates_until_present_frame`, mas renomear/ajustar para a nova semântica se `PresentFrame` virar `PrepareFrame` + `FinishFrame`.

### 2. ACK igual Hyprland: promover maior serial `<= ack`

Problema: ACK serial exato perde o resize se o cliente ACKa um serial posterior que inclui estados anteriores.

Patch sugerido:

```rust
// src/compositor/mod.rs
fn ack_xdg_surface_configure(&mut self, surface_id: u32, serial: u32) {
    let resize = self
        .sent_resize_commits
        .iter()
        .filter_map(|((sent_surface_id, sent_serial), resize)| {
            (*sent_surface_id == surface_id && *sent_serial <= serial).then_some((*sent_serial, *resize))
        })
        .max_by_key(|(sent_serial, _)| *sent_serial)
        .map(|(_, resize)| resize);

    self.sent_resize_commits
        .retain(|(sent_surface_id, sent_serial), _| {
            *sent_surface_id != surface_id || *sent_serial > serial
        });

    if let Some(resize) = resize {
        self.pending_resize_commits.insert(surface_id, resize);
    }
}
```

Teste mínimo:

```rust
#[test]
fn ack_configure_promotes_latest_resize_commit_at_or_before_serial() {
    let mut state = CompositorState::default();
    let surface_id = 42;
    let older = PendingResizeCommit {
        serial: 7,
        width: 300,
        height: 200,
        placement: SurfacePlacement::root_at(0, 0),
        edges: ResizeEdges::BOTTOM_RIGHT,
    };
    let latest = PendingResizeCommit {
        serial: 9,
        width: 340,
        height: 230,
        placement: SurfacePlacement::root_at(10, 20),
        edges: ResizeEdges::BOTTOM_RIGHT,
    };

    state.sent_resize_commits.insert((surface_id, older.serial), older);
    state.sent_resize_commits.insert((surface_id, latest.serial), latest);
    state.ack_xdg_surface_configure(surface_id, 10);

    assert_eq!(state.pending_resize_commits.get(&surface_id), Some(&latest));
    assert!(state.sent_resize_commits.keys().all(|(id, serial)| *id != surface_id || *serial > 10));
}
```

### 3. Não esticar buffer antigo durante resize interativo

Hoje o Oblivion não avança `render_generation` no configure-only resize, então ele não estica automaticamente só por enfileirar configure. O risco aparece quando for implementada visualização de resize em tempo real: se `surface.width/height` virar o tamanho alvo antes do commit do cliente, `blit_surface_to_rect` escala o buffer antigo (`src/compositor/render.rs:699-723`).

Patch de arquitetura sugerido:

- Manter `RenderableSurface.width/height` como tamanho commitado da surface.
- Adicionar estado separado de janela, por exemplo `WindowRenderTarget { box_width, box_height, interactive_resizing }`.
- Renderizar área/moldura da janela pelo target, mas blitar o buffer usando o tamanho commitado enquanto `interactive_resizing == true`.
- Só atualizar `RenderableSurface.width/height` no commit do cliente.

Teste mínimo:

- Criar um root surface 300x200.
- Iniciar resize para 340x230 sem commit novo.
- Renderizar cena com target visual 340x230.
- Verificar que pixels do buffer do cliente só ocupam 300x200 e que a área extra não contém uma versão escalada do conteúdo antigo.

### 4. Cursor de hardware e fallback software damage-only

Problema: todo movimento de mouse redesenha frame completo. Em 1920x1080@165, isso compete diretamente com commits de Gecko.

Patch de fases sugerido:

1. `NativeCursorBackend::try_hardware_cursor(...)`:
   - Criar buffer pequeno de cursor.
   - Usar cursor plane/DRM cursor API quando disponível.
   - Em movimento, chamar apenas move cursor.
2. Fallback software:
   - Guardar `old_cursor_rect` e `new_cursor_rect`.
   - Marcar damage só nessas regiões.
   - Evitar `effect.request_redraw()` para mouse move quando cursor de hardware está ativo e não há window interaction.

Teste mínimo:

- Unidade: `handle_pointer_motion_delta` deve produzir `redraw_requested == false` quando hardware cursor está ativo e não há interação.
- Integração nativa: mover mouse sobre desktop sem clients não deve chamar `paint_server_frame` por evento, apenas mover cursor.

### 5. Damage tracking no native renderer/scanout

Problema: `NativeGbmScanout::paint_server_frame` sempre renderiza/copia o frame inteiro (`src/native_output.rs:2801-2824`). Já existem `RenderableSurfaceDamage` e leitura parcial de SHM (`src/compositor/surface.rs:40-90`, `src/compositor/shm.rs:97-114`), mas o output não usa isso para limitar composição/cópia.

Patch de fases sugerido:

- Adicionar `OutputDamage` no compositor:
  - surface commit: damage transformado para coordenadas de output.
  - placement/move: old box + new box.
  - cursor software: old cursor box + new cursor box.
  - shell overlay: full/topbar/dock regions conforme geração do shell.
- `DesktopSceneRenderer::compose_request` aceitar `damage: Option<&[Rect]>`.
- `NativeGbmScanout` copiar/escrever só regiões danificadas quando o backend permitir. Se `gbm::BufferObject::write` exigir buffer inteiro, considerar BO mapeável ou EGL/GLES render com scissor.

Teste mínimo:

- Commit parcial em SHM deve gerar damage menor que a surface inteira.
- Movimento de cursor software deve gerar duas regiões pequenas.
- Resize/move deve danificar old+new window boxes.

### 6. Mode selection com candidatos testáveis

O Oblivion já parseia `OBLIVION_ONE_MODE=1920x1080@165`, `highrr`, `highres` etc. O ponto fraco é selecionar um único modo (`src/native_output.rs:2449-2491`) e só descobrir erro no `set_crtc`.

Patch sugerido:

- Trocar `select_kms_mode(...) -> Option<mode>` por `kms_mode_candidates(...) -> Vec<mode>`.
- Para `highrr`, ordenar por `(vrefresh, area)` desc e tentar top 3.
- Para `highres`, ordenar por `(area, vrefresh)` desc e tentar top 3.
- Para exact, ordenar por proximidade `(abs(width), abs(height), abs(refresh))`, com exact primeiro.
- No `select_kms_target`, tentar modo+CRTC em sequência e, se houver API de test-only disponível, testar antes de fixar o target.

Teste mínimo:

- `kms_mode_candidates_highrr_keeps_top_three_in_priority_order`.
- `kms_mode_candidates_exact_1920_1080_165_prefers_exact_then_nearest_refresh`.
- `select_kms_target_falls_back_when_first_mode_rejected` com backend/trait fake, se o código for extraído para testabilidade.

### 7. Scheduler nativo orientado por pageflip/vblank

Problema: o loop atual usa `thread::sleep` e drena pageflip no começo da iteração (`src/native_output.rs:344-390`). Hyprland dirige render por `frame/needsFrame/present` e usa explicit sync para render/commit no timing certo.

Patch de arquitetura sugerido:

- Transformar `NativeGbmScanout::drain_page_flip_events` para retornar evento com timestamp/seq quando possível.
- Chamar `server.finish_frame_with_presentation(timestamp, refresh, seq)` após pageflip concluído.
- Só renderizar novo frame quando:
  - há damage/pending protocol work;
  - pageflip anterior concluiu ou há buffer livre;
  - vblank deadline permite.
- Manter timer apenas como fallback.

Teste mínimo:

- Simular pageflip pending: nova chamada de render não deve completar frame callbacks antes do evento de pageflip.
- Simular pageflip completion: callbacks e presentation feedback devem ser completados uma vez.

## Riscos/validação

- **ACK `<= serial`**: baixo risco e alta compatibilidade com Hyprland. Validar com os testes atuais e o teste novo de ACK serial posterior.
- **Separar `present_frame`**: risco médio. Pode mudar timing de frame callbacks e releases. A validação deve incluir clientes SHM simples, Firefox/Zen, popup/menus e explicit sync.
- **Callbacks no pageflip real**: risco médio/alto, mas é o caminho correto para NVIDIA e alta taxa. Precisa validar que não há deadlock quando pageflip falha ou backend dumb framebuffer está ativo.
- **Hardware cursor**: risco alto em diversidade de drivers/planes, especialmente NVIDIA. Deve ter fallback software automático e logs claros.
- **Damage tracking**: risco alto de regiões stale se algum overlay/blur/shell não marcar damage. Começar com damage conservador por janela inteira + cursor boxes; só depois reduzir.
- **Não esticar buffer antigo**: risco médio. Precisa preservar viewport/fractional scale legítimos. A regra deve ser ligada a resize interativo de toplevel, não a qualquer diferença buffer/target.
- **Mode fallback**: baixo/médio. Evita falhas de modo, mas pode escolher fallback inesperado. Logs devem imprimir lista de candidatos e motivo de rejeição.

Validação manual recomendada:

1. Rodar com `OBLIVION_ONE_MODE=1920x1080@165` e confirmar log do modo escolhido.
2. Abrir Zen/Firefox/Gecko em native backend.
3. Redimensionar bottom-right e top-left por 10 segundos.
4. Medir se configure/ack/commit seguem o mouse:
   - log de `send_resize_configure_to(surface, width, height, serial)`;
   - log de `ack_xdg_surface_configure(surface, serial)`;
   - log de commit com tamanho do buffer/surface.
5. Mover mouse sobre desktop sem interação e verificar se não há repaint total quando hardware cursor está ativo.
6. Em fallback software cursor, verificar damage restrito às caixas old/new do cursor.
7. Confirmar que frame callbacks/presentation feedbacks não são emitidos antes do pageflip quando GBM pageflip está ativo.

Root cause provável final:

O problema visível de resize em Zen/Gecko no native não parece ser uma única falha de protocolo isolada. A árvore atual já tem pending resize serializado e `pending_resize_configure` conta como frame work. O gargalo principal é que o backend nativo envia resize configure e frame callbacks dentro de uma fase monolítica (`present_frame`) que acontece depois da pintura/present, enquanto mouse move força composição/cópia de frame inteiro. O ACK exato é um segundo bug provável para clientes que coalescem ACKs ou ACKam serial posterior. Hyprland evita essa combinação com ACK `<= serial`, damage-based render, cursor de hardware/fallback damage-only e frame scheduling amarrado ao output.
