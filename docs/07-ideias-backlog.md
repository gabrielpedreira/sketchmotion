# Backlog de ideias

Ideias levantadas durante o desenvolvimento, para implementar **fora da ordem**
do roadmap principal ([`05-roadmap-mvp.md`](./05-roadmap-mvp.md)). Cada uma é
detalhada e encaixada numa versão quando for priorizada — registrar aqui evita
perder a ideia sem interromper o trabalho em andamento.

## Em andamento

### Paletas por personagem (crate `color`)
Paletas nomeadas por área do personagem (Pele, Roupa, Cabelo...) com cores de
rótulo livre (base, luz, sombra...) e código hex, para manter consistência do
personagem entre frames e entre projetos.
- [x] Parte A — estrutura de dados + hex + testes
- [x] Parte B — seletor de cores abrangente (clique + código hex)
- [ ] Parte C — painel de gerenciamento (criar personagem/grupo/cores, pintar a partir da paleta)
- [ ] Parte D — persistência (biblioteca global reutilizável + embutida no projeto)

## Backlog (ainda não iniciado)

### Modo pixel art
Canvas em baixa resolução de pixels lógicos escolhida pelo usuário
(ex.: 16/32/64 px = nível de detalhamento), exibido ampliado com **grade**
sobreposta guiando as células; lápis pinta **1 pixel lógico** por clique
(encaixado na grade, sem antisserrilhado).

### Tipos de pincéis e texturas
Ampliar o sistema de pincéis (crate `brush`) para além do lápis sólido:
- Pincéis de tinta/nanquim (inking), com variação de espessura no traço.
- Aquarela / pincéis com esmaecimento (opacidade e bordas suaves).
- Spray / aerógrafo (dispersão de pontos).
- Pincéis com **textura** (a "ponta" do pincel é uma imagem/máscara aplicada ao longo do traço).
- Arquitetura deve permitir **adicionar novos pincéis** sem alterar os existentes
  (cada pincel implementa um trait comum), e futuramente importar pincéis.

### Toolbox à direita (painéis mostráveis/ocultáveis)
Uma barra/coluna à direita que agrupa painéis que o usuário pode **exibir ou
ocultar** individualmente, no estilo Illustrator:
- Paletas personalizadas (por personagem)
- Camadas
- Seleção de cores (com amostras visuais, não só código hex)
- Degradês
- Opacidade
Cada painel é um módulo independente ligado/desligado por ícones. Substitui o
painel lateral fixo atual por algo configurável.

### Melhoria na criação de paleta
Na área de criação da paleta, mostrar **amostras de cor** (blocos visuais),
não depender só do código; ao salvar, a amostra vai para o painel/janela de
paletas personalizadas.

### Camadas nomeadas por elemento (ligado a camadas e animação)
Criar camadas e **nomear cada elemento** do personagem (chapéu, cabeça,
roupa...). Assim, num frame diferente, o usuário seleciona um elemento e o
**move inteiro ou o edita** conforme a cena. Base para reaproveitar partes do
personagem entre frames — conecta o sistema de camadas (v0.2) com a animação
(v0.3) e o rigging (v0.5).

### Barra de ferramentas à esquerda (estilo Illustrator)
Uma coluna vertical de ferramentas no lado esquerdo, separada da toolbox de
painéis (direita). Ferramentas previstas (baseadas no Illustrator):
seleção, laço/seleção livre, **caneta ponto a ponto** (pen/bézier), lápis,
pincel, borracha, **conta-gotas**, **balde de preenchimento**, formas
(linha, retângulo, elipse), texto, mão (pan) e zoom. Cada ferramenta é um
botão/ícone; a arquitetura (crate `tools`) deve permitir adicionar novas sem
mexer nas existentes. No rodapé, os seletores de cor de preenchimento/traço.

### Ferramentas da barra esquerda ainda sem lógica (stubs → etapas próprias)
A barra de ferramentas à esquerda já foi construída (estrutura + ícones) e a
barra de opções no topo (controles da ferramenta ativa). Já funcionam: Pincel,
Borracha e Conta-gotas. Entraram como ícone, mas **sem lógica**, aguardando
etapa dedicada (cada uma é um módulo/estado próprio no crate `tools`):

- **Seleção (seta)** — selecionar um elemento/região e movê-lo. Depende do
  sistema de seleção (retângulo/mover conteúdo). Base da v0.2.
- **Seleção direta** — editar por pontos. Depende de um modelo vetorial/pontos
  (hoje o canvas é raster); vem junto com Caneta.
- **Varinha mágica** — selecionar pixels da mesma cor (flood/contíguo ou global)
  a partir de um clique. Depende do sistema de seleção.
- **Caneta (pen)** — desenhar por pontos/bézier (traçados vetoriais). Módulo
  vetorial novo.
- **Texto** — inserir e editar texto no canvas (fonte, tamanho, cor). Módulo de
  texto novo; a espessura/tamanho vai na barra de opções do topo.

Ordem sugerida quando forem implementadas: Seleção (mover) → Varinha mágica →
Caneta → Seleção direta → Texto.
