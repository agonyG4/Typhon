# Agent Raman: Hyprland Resize/Scale Follow-up

## Escopo

Pesquisa read-only sobre o estado atual do Oblivion depois do trabalho de
resize preview, comparando com a referencia local em
`WM para Referencia/Hyprland-main`.

Pergunta guia: quais otimizacoes de resize/scale ainda faltam para o Oblivion
ficar mais parecido com Hyprland sem esticar buffer velho de Gecko
Firefox/Zen?

## Resumo

O Oblivion ja saiu do estado documentado nas pesquisas antigas em que resize
configure nao mudava visual ate o proximo commit do cliente. O estado atual
pós-preview:

- cria `ResizePreview` no `RenderableSurface`;
- avanca `render_generation` com causa `WindowResize` durante o drag;
- renderiza o conteudo antigo dentro do tamanho comprometido, sem fazer upscale
  do buffer pequeno para preencher o alvo novo;
- ancora right/bottom em resizes pela borda left/top;
- promove ACK de resize pelo maior serial `<= ack_serial`;
- calcula damage old/new bounds para `WindowMove`, `WindowResize` e
  `SurfacePlacement` no backend nativo.

As lacunas que ainda separam o caminho atual do modelo Hyprland nao parecem
mais ser a geometria basica de preview. Elas estao principalmente em:

1. pacing temporal do resize contra o refresh do monitor;
2. separacao entre "enviar configure" e "completar frame callbacks";
3. callbacks de frame no caminho sem damage, importante para Firefox;
4. semantics mais completas de surface pass, viewporter e fractional scale;
5. propagacao de scale para subtrees/popups como Hyprland faz;
6. dano/copia ainda conservadores quando o scene renderer decide fazer rebuild
   completo.

## Estado Atual do Oblivion

### Resize preview ja existe

`RenderableSurface` agora carrega `resize_preview`:

- `src/compositor/surface.rs:6-17` define `RenderableSurface` com
  `resize_preview`.
- `src/compositor/surface.rs:41-47` define `ResizePreview` com
  `committed_width`, `committed_height`, `anchor_right` e `anchor_bottom`.

O caminho de resize interativo agora aplica visual state antes do cliente
commitar:

- `src/compositor/mod.rs:2284-2311` coalesce a configure pendente e chama
  `preview_resize_root_window_to`.
- `src/compositor/mod.rs:2314-2365` atualiza placement/size do surface
  renderizavel, cria `ResizePreview`, marca `WindowResize` e poe damage full
  na surface.
- `src/compositor/tests/windows.rs:240-269` confirma que drag resize ja altera
  o target renderizavel antes do commit do cliente.
- `src/compositor/tests/windows.rs:436-450` confirma que configure-only resize
  avanca `render_generation` para preview.

Isso e uma mudanca importante em relacao a documentos anteriores: nao devemos
mais recomendar "fazer configure-only resize avancar geracao" como trabalho
pendente; isso ja aconteceu.

### Nao esticar o buffer antigo ja esta coberto no caminho CPU

O blit de surface chama `resize_preview_content_target` antes de desenhar:

- `src/compositor/render.rs:1053-1065` ajusta o target antes do blit.
- `src/compositor/render.rs:1146-1168` reduz o target do conteudo para a
  extensao comprometida e aplica ancoragem right/bottom.
- `src/compositor/render.rs:1171-1179` limita a extensao preview entre `1` e o
  target.
- `src/compositor/render.rs:1465-1507` testa que preview nao faz upscale de um
  committed buffer menor.
- `src/compositor/tests/windows.rs:526-555` testa shrink pela borda esquerda
  mantendo a borda direita ancorada.

A regra atual e parecida com o detalhe que importa para Gecko: a moldura/alvo
pode acompanhar o ponteiro, mas o conteudo velho nao deve ser inflado para o
tamanho novo antes de um commit compativel.

### Commit real substitui o preview

No commit de buffer:

- `src/compositor/mod.rs:872-977` resolve placement pendente, aplica buffer e
  atualiza a surface renderizavel.
- `src/compositor/mod.rs:2470-2495` so consome o resize ACKed se o tamanho
  commitado combina com o resize esperado; se nao combinar, reinsere o pending
  resize.
- `src/compositor/mod.rs:2937-2964` limpa `resize_preview` quando um buffer real
  substitui o estado preview.
- `src/compositor/mod.rs:2497-2511` promove o resize mais recente cujo serial
  enviado e `<= ack_serial`, alinhado com Hyprland.

Esse e o ponto que evita o bug classico de Gecko: ACK nao e tratado como
conteudo novo; o commit compativel e que substitui o visual preview.

### Damage old/new bounds ja existe para resize

O backend nativo agora tem damage por bounds antigo/novo:

- `src/native_output.rs:3547-3592` constroi damage comparando bounds anteriores
  e atuais por `surface_id`.
- `src/native_output.rs:3640-3673` usa esse caminho para `WindowMove`,
  `WindowResize` e `SurfacePlacement`; caso nao consiga produzir rects, cai
  para full output.
- `src/native_output.rs:4914-4947` testa old/new bounds em move.
- `src/native_output.rs:4950-4981` testa old/new bounds em resize.

Ainda ha custo conservador quando o scene renderer precisa reconstruir tudo:

- `src/native_output.rs:866-883` pinta e apresenta o frame com o damage
  calculado.
- `src/native_output.rs:914-915` so atualiza o snapshot anterior depois do
  repaint.
- `src/native_output.rs:4985+` cobre fallback de full copy quando o scene
  rebuild e full.

Portanto, old/new damage nao e mais uma lacuna conceitual, mas ainda precisa
ser validado em casos com left/top resize, scale fracionaria, popups e shells.

### Input coalescing existe, throttle temporal nao

O Oblivion ja coalesce eventos consecutivos antes de processar input:

- `src/native_output.rs:784-789` drena raw events e chama
  `coalesce_pointer_motion_events`.
- `src/native_output.rs:2764-2815` soma motion relativo consecutivo e troca
  absolute motion pela posicao mais recente.
- `src/native_output.rs:5787-5835` testa coalescing relativo, boundaries de
  botao e absolute motion.

Mas durante uma interacao de janela, cada motion coalescido ainda vira update
visual:

- `src/native_output.rs:1638-1656` faz pointer motion gerar
  `UpdateInteraction` e `request_visual_redraw` quando ha window interaction.
- `src/native_output.rs:2972-3017` aplica os efeitos de input.
- `src/native_output.rs:3026-3048` chama `server.update_window_interaction`.
- `src/compositor/mod.rs:1906-1940` calcula geometria de resize e chama
  `queue_resize_root_window_to`.

O que falta aqui e o equivalente ao throttle temporal do Hyprland: limitar
updates de resize por cadencia de monitor, sem perder o ultimo evento e sem
throttlar move da mesma forma.

### Frame/present ainda sao monoliticos

O `present_frame` do servidor ainda mistura varias responsabilidades:

- `src/compositor/server.rs:226-234` commita explicit sync pronto, flusha color,
  flusha pending resize configure, libera buffers, completa frame callbacks e
  presentation feedbacks.
- `src/native_output.rs:929-939` chama `server.present_frame()` somente depois
  do repaint/present quando ainda ha pending frame work.
- `src/compositor/mod.rs:2803-2811` completa callbacks pendentes pegando todos
  os callbacks em surfaces conhecidas.
- `src/compositor/mod.rs:2826-2830` considera pending resize configure,
  callbacks e presentation feedbacks como frame work.

Isso entrega progresso, mas e menos Hyprland-like que um ciclo com:

1. prepare/protocol flush antes de decidir/pintar o frame;
2. repaint somente quando ha damage ou visual change;
3. completion de callbacks/presentation amarrada a presented/no-damage path.

### Fractional scale existe, mas e global e pouco integrado ao render path

O protocolo fracionario basico esta presente:

- `src/compositor/output.rs:33-60` converte fator para denominator `120` e usa
  ceil para `wl_output.scale`.
- `src/compositor/mod.rs:289-298` envia `wl_output.scale` e preferred fractional
  scale quando o output scale muda.
- `src/compositor/mod.rs:409-421` registra `wp_fractional_scale_v1` e manda o
  preferred scale atual.
- `src/compositor/mod.rs:436-441` manda scale para recursos fracionarios ja
  bound.
- `src/compositor/protocols/viewport.rs:40-49` aceita viewport destination.
- `src/compositor/tests/input_output.rs:103-114` cobre update para escala 1.5.

Mas o render nativo ainda chama compose com `output_scale: 1.0`:

- `src/native_output.rs:1110-1122` monta `DesktopComposeRequest` com
  `output_scale: 1.0`.

Tambem nao encontrei, neste ciclo, um equivalente Hyprland para scale conhecido
por subtree/popup e correcao de misalignment FSV1 no render path. Isso importa
para Firefox/GTK/Gecko porque menus e popups podem envolver conteudo em
subsurfaces com `wp_viewport`.

## Hyprland: Comportamentos de Referencia

### Throttle de pointer resize

Hyprland nao aplica todo motion bruto ao resize. Ele calcula delta desde o
inicio e desde o ultimo tick, compara com o periodo do monitor e pode pular
updates:

- `WM para Referencia/Hyprland-main/src/layout/supplementary/DragController.cpp:254-261`
  calcula `DELTA`, `TICKDELTA` e `MSMONITOR`.
- `DragController.cpp:270-279` acumula media temporal e retorna cedo quando o
  update chega antes do periodo do monitor e pode ser pulado.
- `DragController.cpp:281-287` so depois disso atualiza `m_lastDragXY` e
  danifica o target.
- `DragController.cpp:306-370` aplica resize floating ou tiled.

Takeaway: o Oblivion tem coalescing por batch de evdev, mas nao tem pacing por
refresh. Para resize, isso ainda pode produzir configures/previews demais em
mouse de alta taxa.

### Damage antes/depois da geometria

Hyprland marca dano de forma redundante e conservadora ao redor da mudanca:

- `DragController.cpp:285` chama `damageEntire` antes de mover/redimensionar.
- `DragController.cpp:387` chama `damageEntire` depois.
- `WM para Referencia/Hyprland-main/src/layout/target/WindowTarget.cpp:40-42`
  danifica a janela antes e usa scope guard para danificar depois.
- `WindowTarget.cpp:50-58` atualiza pos/size real e chama `sendWindowSize` para
  floating.
- `WM para Referencia/Hyprland-main/src/render/Renderer.cpp:2754-2766` converte
  window bounds para damage por monitor e scale.

Takeaway: o Oblivion ja tem old/new bounds no nativo. O proximo cuidado e
garantir que todos os caminhos que mudam target visual, including left/top
resize e scale != 1, alimentem rects corretos e nao caiam frequentemente em
full output por rebuild completo.

### Surface pass evita inflar surface pequena no resize interativo

O detalhe Gecko relevante esta no surface pass:

- `WM para Referencia/Hyprland-main/src/render/pass/SurfacePassElement.cpp:26-31`
  detecta resize interativo e constroi o window box.
- `SurfacePassElement.cpp:36-50` quando a surface e menor que o viewport
  esperado, o caminho nao-interativo corrige/translada e escala; no caminho de
  resize interativo, usa `SIZE.x`/`SIZE.y`, evitando preencher o alvo maior com
  textura velha.
- `WM para Referencia/Hyprland-main/src/desktop/view/WLSurface.cpp:40-50`
  define `small()` como reported size maior que current texture size.
- `WLSurface.cpp:53-79` calcula vetores e tamanho corrigidos por viewporter.

Takeaway: o `ResizePreview` do Oblivion resolve a versao CPU simples do mesmo
problema. O que ainda falta e a generalizacao: diferenciar resize preview,
viewport source/destination, buffer scale e fractional-scale misalignment sem
reaproveitar a mesma regra de scale para tudo.

### Edge expansion e FSV1 sao separados do resize interativo

Hyprland ainda tem ajustes de UV/edge para casos legitimos:

- `WM para Referencia/Hyprland-main/src/render/ElementRenderer.cpp:53-110`
  lida com viewport source, surfaces menores que viewport e
  `render:expand_undersized_textures`.
- `ElementRenderer.cpp:238-265` detecta resize interativo ao calcular o caminho
  de fractional-scale misalignment.
- `ElementRenderer.cpp:259-263` desativa o fast path FSV1 quando janela esta
  animando ou em resize interativo.
- `ElementRenderer.cpp:272-277` usa nearest neighbor para misalignment FSV1
  pequeno fora desses casos.

Takeaway: a regra "nao esticar buffer velho" deve continuar presa ao resize
interativo de toplevel. Viewporter/fractional scale podem precisar de stretch,
crop ou UV fixup legitimos.

### Firefox e frame callbacks sem damage

Hyprland tem um caminho explicito para Firefox quando nao ha damage:

- `WM para Referencia/Hyprland-main/src/output/Monitor.cpp:163-175` comenta que
  Firefox pode esperar novo frame callback quando nada agenda frames; se o
  monitor nao deve renderizar, Hyprland envia frame events para workspaces
  visiveis no present.
- `WM para Referencia/Hyprland-main/src/render/Renderer.cpp:2200-2205` tambem
  envia frame callbacks no caminho sem damage.
- `Renderer.cpp:2504-2510` envia callbacks para views vivas/visiveis do
  workspace.

Takeaway: hoje o Oblivion trata pending frame callback como motivo para frame
work e `present_frame` completa callbacks de maneira ampla. Isso pode manter
clientes andando, mas nao e o mesmo que "no-damage frame callback no presented"
sem forcar repaint ou completar callback cedo demais.

### Configure batching e ACK

Hyprland evita spam e aceita ACK coalescido:

- `WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp:1633-1655`
  evita reenviar size igual, guarda `(serial, size)` e envia `setSize`.
- `Window.cpp:1414-1428` escolhe o resize pendente mais recente cujo serial e
  `<= ack_serial`, remove anteriores e aplica acked size.
- `WM para Referencia/Hyprland-main/src/layout/target/WindowTarget.cpp:57`,
  `:100` e `:227` chamam `sendWindowSize` depois de atualizar target.

Takeaway: o Oblivion ja copiou o comportamento critico de ACK `<=`. O que ainda
falta e pacing/batching temporal para nao transformar todo motion coalescido em
configure/preview quando o output ainda nao chegou ao proximo tick.

### Fractional scale por arvore de surfaces

Hyprland manda scale de forma mais contextual:

- `WM para Referencia/Hyprland-main/src/protocols/core/Output.cpp:72-79` manda
  `wl_output.scale = ceil(monitor.scale)`.
- `WM para Referencia/Hyprland-main/src/protocols/FractionalScale.cpp:58-62`
  guarda scale conhecido por surface e notifica addon se existir.
- `FractionalScale.cpp:73-79` envia preferred scale como `round(scale * 120)`.
- `WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp:480-492` caminha
  breadth-first pela arvore da window e manda fractional/preferred scale e
  transform.
- `WM para Referencia/Hyprland-main/src/desktop/view/Popup.cpp:461-477` faz
  algo parecido para popups; o comentario cita Firefox/GTK e subsurfaces com
  `wp_viewport`.
- `WM para Referencia/Hyprland-main/src/protocols/core/Subcompositor.cpp:202-207`
  herda scale/transform conhecidos para subsurfaces novas.

Takeaway: para ficar parecido com Hyprland em Gecko/GTK, scale nao pode ser so
um broadcast global para recursos fracionarios ja bound. Precisa haver nocao de
scale conhecido por surface tree, especialmente para popup/subsurface criado
depois.

## Lacunas Priorizadas

### Aplicar agora

1. Throttle temporal de resize interativo.

   Implementar um estado pequeno por drag no native/input ou compositor:
   ultimo update aplicado, ultimo ponteiro acumulado, periodo estimado pelo
   refresh atual. Para resize, permitir no maximo um update por periodo de
   monitor, mantendo o ultimo ponteiro pendente para flush no release. Nao
   aplicar a mesma regra a move sem avaliar, porque Hyprland exclui `MBIND_MOVE`
   da parte principal do skip em `DragController.cpp:278`.

2. Flush de resize configure antes da pintura.

   Separar uma fase "prepare frame" de `present_frame`: flush de color/resize
   configure e explicit-sync-ready antes de decidir/pintar; completion de frame
   callbacks, buffer release e presentation depois do pageflip/no-damage. Isso
   reduz a chance de o preview local ir um frame na frente do configure que o
   Gecko ainda nem recebeu.

3. No-damage frame callback path para clientes visiveis.

   Adicionar um caminho parecido com Hyprland: se nao ha damage para renderizar
   mas ha frame callbacks em surfaces visiveis, entregar callbacks no tick de
   present/no-damage sem full repaint. Comecar conservador: apenas surfaces
   mapped/visiveis no workspace atual; nao callbacks globais de qualquer
   resource.

4. Testar mais resizes de borda com damage old/new.

   O damage old/new existe, mas os testes atuais cobrem move e resize simples.
   Adicionar casos de shrink pela esquerda/topo e rects escalados quando output
   scale != 1, porque esses sao os cenarios que mais mostram trail/ghost.

5. Instrumentar resize pacing.

   O `NativeResizePerfState` ja conta updates em
   `src/native_output.rs:149-205`. Expandir logs com `raw_input_events`,
   `coalesced_input_events`, updates aplicados vs pulados, serial/configure
   enviado e damage pixels. Isso deixa claro se throttle melhora Zen sem
   esconder um under-damage.

### Aplicar depois

1. Surface pass com semantica Hyprland-like completa.

   O `ResizePreview` e bom para CPU/simple shm. Depois, separar explicitamente:
   committed content size, requested toplevel size, viewport source/destination,
   buffer scale, output scale e fractional-scale fixups. A regra de nao esticar
   deve continuar condicionada ao resize interativo de toplevel.

2. Fractional scale por surface tree e popups.

   Criar cache de preferred scale/transform conhecido por surface, aplicar em
   subtree breadth-first, herdar para subsurfaces novas e tratar popups. Esse e
   o paralelo direto com Hyprland para Firefox/GTK com subsurfaces em
   `wp_viewport`.

3. Frame scheduler/pageflip de verdade.

   Migrar de `present_frame` monolitico para lifecycle orientado por output:
   render no frame event quando ha damage, callbacks/presentation no presented,
   no-damage callbacks sem repaint, e timer apenas como fallback.

4. Damage/copy menos conservador apos scene rebuild.

   Hoje o output damage old/new existe, mas a copia pode virar full quando o
   scene rebuild e full. Depois de estabilizar resize/pacing, reduzir rebuild
   completo nos casos em que so bounds de uma surface mudam.

5. GPU/DMABUF real.

   O estudo aqui e resize/scale; ainda assim, o caminho Hyprland final depende
   de importar texturas/dmabufs e aplicar damage no renderer GPU. Manter resize
   preview e fractional scale corretos antes evita levar o bug de stretch para o
   futuro backend GPU.

## Testes Sugeridos

### Unit/integration

1. `resize_throttle_limits_updates_to_refresh_period`

   Com clock fake, simular 100 motion events em menos de um periodo de 60 Hz.
   Esperado: um update visual aplicado, ultimo ponteiro pendente preservado.

2. `resize_throttle_flushes_pending_pointer_on_release`

   Arrastar rapido e soltar antes do proximo tick. Esperado: resize final usa a
   ultima posicao do ponteiro, e `send_resize_end_configure` nao perde tamanho.

3. `resize_preview_left_top_no_upscale_and_old_new_damage`

   Fazer shrink por left/top antes do commit do cliente. Esperado: conteudo
   velho nao estica, origem/ancora corretas, damage contem old e new bounds.

4. `resize_preview_ack_without_matching_commit_keeps_preview`

   ACK de configure seguido de damage-only/same-size commit. Esperado:
   `resize_preview` permanece e o conteudo velho nao ocupa o alvo novo.

5. `visible_frame_callback_no_damage_does_not_force_full_repaint`

   Surface visivel pede `wl_surface.frame` sem damage novo. Esperado: callback e
   completado no caminho no-damage e `native.frame` nao reporta full repaint por
   causa apenas do callback.

6. `fractional_scale_popup_subsurface_inherits_preferred_scale`

   Criar toplevel em 1.5, popup/subsurface depois. Esperado: root, popup e
   subsurface recebem `preferred_scale = 180`, sem depender apenas de recursos
   ja registrados antes do scale change.

7. `viewport_destination_resize_preview_does_not_mix_physical_logical_size`

   Surface com `wp_viewport.set_destination`, buffer scale e resize preview.
   Esperado: destino logico nao vira tamanho fisico, e a regra no-stretch so
   afeta resize interativo.

### Manuais

1. Zen/Firefox native, scale 1.0

   Abrir Zen/Firefox, redimensionar rapido por bottom/right, left e top. Validar
   que a moldura segue o ponteiro, conteudo velho nao estica e o commit real
   substitui o preview sem salto.

2. Zen/Firefox native, scale 1.25 ou 1.5

   Repetir com fractional scale. Validar menus/popups, hit testing e ausencia
   de blur/offset por subsurface em `wp_viewport`.

3. Chromium/Brave native

   Repetir resize para garantir que throttle e no-damage callbacks nao pioram
   clientes que costumam responder mais rapido a resize configures.

4. Perf logs durante resize

   Capturar `native.frame`, `resize.update` e `native.present_frame`. Comparar:
   raw input vs coalesced input, updates aplicados vs pulados, `damage_kind`,
   `damage_rects`, `damaged_pixels`, `render_cause=window_resize`.

5. Sem motion visual

   Mover mouse sobre janela sem drag com cursor hardware ativo. Esperado:
   ausencia de repaint de frame inteiro; somente input forwarding/cursor plane.

6. No-damage Firefox callback

   Em Firefox/Zen parado, forcar caso com pending frame callback sem damage
   visual novo. Esperado: callback progride sem `native.frame` full repaint.

## Conclusao

O Oblivion pos-preview ja tem a parte mais perigosa do resize Gecko: separar
alvo de resize do conteudo commitado e nao fazer upscale do buffer velho. Para
ficar mais parecido com Hyprland, a proxima rodada deve focar menos em
geometria basica e mais em cadencia:

- throttle temporal de resize contra refresh;
- configure flush antes da pintura;
- frame callbacks visiveis no caminho sem damage;
- scale/fractional/viewport por arvore de surfaces;
- validacao de old/new damage em left/top e scale fracionaria.

Essa ordem libera suavidade sem reintroduzir o stretch de Gecko.
