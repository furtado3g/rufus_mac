# Feature Specification: M5 — Windows installation media

**Created**: 2026-10-21

**Status**: Draft

**Input**: User description: "Implementar M5 do ROADMAP.md — Windows installation media"

## Clarifications

### Session 2026-10-21

- Q: O M5 deve tratar arquivos grandes apenas dividindo WIM em FAT32, deixando NTFS e UEFI:NTFS para depois? → A: Não. Incluir FAT32 com WIM dividido e NTFS com UEFI:NTFS neste M5.
- Q: Qual opção deseja oferecer quando o Secure Boot impedir a instalação? → A: Oferecer separadamente ignorar a exigência do instalador Windows 11 e orientar a desativação manual no firmware quando necessária, ambas desativadas por padrão; nunca alterar o firmware automaticamente.
- Q: Como escolher entre FAT32 com WIM dividido e NTFS com UEFI:NTFS? → A: Automaticamente priorizar FAT32, dividindo WIM quando necessário, e usar NTFS para arquivos acima do limite FAT32 que não possam ser divididos, como ESD; permitir escolha manual em CLI e GUI.
- Q: Para concluir o M5, é obrigatório validar boot com Secure Boot ativado nos dois caminhos? → A: Sim. Validar FAT32 e NTFS com UEFI:NTFS com Secure Boot ativado e desativado, em firmware compatível com as assinaturas distribuídas.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Criar mídia de instalação Windows (Priority: P1)

Como usuário no macOS, Linux ou Windows, quero selecionar uma ISO Windows 10/11 e criar um USB que abra o instalador em um computador UEFI compatível, sem modificar a ISO original.

**Why this priority**: O instalador inicializável é o resultado central do M5.

**Independent Test**: Gravar uma ISO Windows com arquivos abaixo do limite FAT32 em um destino de teste, verificar os arquivos e inicializar até a primeira tela de instalação.

**Acceptance Scenarios**:

1. **Given** uma ISO Windows válida e um dispositivo removível elegível, **When** o usuário confirma a extração, **Then** a mídia resultante abre a interface do instalador no firmware da arquitetura correspondente.
2. **Given** uma ISO sem identificação textual de Windows, mas com `sources/install.wim`, `sources/install.esd` ou `sources/install.swm`, **When** ela é analisada, **Then** é identificada como candidata a mídia Windows; a cadeia de boot é validada antes de qualquer gravação.
3. **Given** uma mídia Windows reconhecida, **When** o usuário usa CLI ou GUI, **Then** ambas apresentam compatibilidade, destino, alterações solicitadas e confirmação de apagamento, sem a antiga rejeição genérica de Windows.
4. **Given** uma operação concluída com verificação solicitada, **When** o destino é relido, **Then** arquivos copiados correspondem à origem e arquivos transformados correspondem ao resultado esperado, ou a operação falha explicitamente.

### User Story 2 - Usar imagens de instalação grandes (Priority: P1)

Como usuário, quero criar a mídia mesmo quando `install.wim` ultrapassa o limite por arquivo do FAT32, sem preparar os arquivos manualmente.

**Why this priority**: Imagens grandes são comuns; copiá-las diretamente produziria uma mídia incompleta.

**Independent Test**: Preparar uma mídia com WIM acima de 4 GiB, confirmar que todas as partes são reconhecidas pelo instalador e que as edições disponíveis são preservadas.

**Acceptance Scenarios**:

1. **Given** um `sources/install.wim` maior que 4.294.967.295 bytes cuja divisão produz partes válidas, **When** a mídia FAT32 é criada, **Then** ele é substituído no destino por `install.swm`, `install2.swm` e demais partes necessárias, cada uma dentro do limite FAT32, preservando todas as imagens de instalação.
2. **Given** uma ISO com ESD que cabe em FAT32 ou um conjunto SWM completo com partes que cabem, **When** a mídia é criada, **Then** esses arquivos são preservados sem conversão desnecessária.
3. **Given** um arquivo grande que deva ser preservado sem divisão, inclusive ESD acima do limite FAT32, **When** o plano usa NTFS com UEFI:NTFS, **Then** o arquivo é copiado integralmente e o instalador reconhece o conteúdo.
4. **Given** partes SWM ausentes ou espaço insuficiente para preparação e destino, **When** o pré-processamento é executado, **Then** a operação informa a causa e a ação corretiva antes de modificar o destino.
5. **Given** um WIM cujo formato ou recurso indivisível impede uma divisão válida, **When** a seleção é automática, **Then** o plano usa NTFS e informa o motivo; com FAT32 explicitamente solicitado, a operação falha antes da gravação.

### User Story 3 - Escolher ajustes Windows 11 explicitamente (Priority: P2)

Como usuário, quero escolher separadamente ignorar verificações de TPM, ignorar verificações de Secure Boot e permitir configuração com conta local, sabendo quais mudanças serão aplicadas.

**Why this priority**: São opções do M5, mas não podem alterar silenciosamente a instalação padrão.

**Independent Test**: Comparar mídias padrão e mídias com cada opção selecionada; conferir que apenas as alterações selecionadas estão presentes e exercitar o comportamento no instalador compatível.

**Acceptance Scenarios**:

1. **Given** opções não selecionadas, **When** a mídia é criada, **Then** nenhuma alteração de registro ou comportamento de instalação é adicionada e os arquivos de boot são preservados, salvo o fallback removível necessário.
2. **Given** uma opção selecionada na CLI ou GUI, **When** o plano é confirmado, **Then** somente o ajuste solicitado é aplicado ao ambiente de instalação; a ISO e o sistema operacional do computador hospedeiro permanecem intactos.
3. **Given** uma imagem incompatível com um ajuste ou uma personalização existente que conflita com ele, **When** a preparação é executada, **Then** a criação é recusada antes de modificar o destino, sem descartar a personalização existente.
4. **Given** uma incompatibilidade conhecida com Secure Boot, **When** o usuário escolhe consultar orientações, **Then** CLI e GUI explicam a desativação manual no firmware, o impacto na proteção de boot e a diferença em relação a ignorar a exigência do instalador, sem alterar o firmware.

### Edge Cases

- ISOs somente UDF precisam ser legíveis para suportar a matriz Windows; uma imagem cujo conteúdo não possa ser lido deve ser recusada antes de gravar, com motivo específico.
- Marcadores Windows isolados não tornam uma imagem inicializável: arquivos de boot ausentes, arquitetura incompatível ou fallback impossível impedem a gravação.
- Nomes e diretórios devem ser reconhecidos sem depender de diferenças de maiúsculas/minúsculas.
- Arquivos de 4.294.967.295 bytes cabem em FAT32; arquivos a partir de 4.294.967.296 bytes exigem tratamento ou rejeição antecipada.
- ESD acima do limite FAT32 e outros arquivos grandes não-WIM exigem NTFS com UEFI:NTFS; um plano FAT32 incompatível é recusado antes de gravar, com orientação para usar NTFS.
- Conjuntos SWM incompletos, conflitantes ou sem sequência válida são recusados; partes existentes nunca são sobrescritas por uma divisão nova.
- WIM corrompido, recurso de preparação indisponível ou espaço temporário insuficiente causa erro antes da gravação.
- Cancelamento, desconexão, falha de leitura/gravação ou divergência de verificação nunca resulta em mensagem de sucesso; um destino parcialmente gravado é informado como incompleto.
- Um destino de arquivo que corresponde à ISO original, inclusive por links, deve ser recusado antes de qualquer alteração.
- Preparações temporárias da operação são removidas após sucesso, erro ou cancelamento normal; arquivos preexistentes do usuário não são removidos.
- No caminho NTFS, geometria interna do asset UEFI:NTFS incompatível ou não comprovada para o setor lógico do destino causa recusa antes da gravação; alinhamento externo não comprova suporte 4Kn. O suporte FAT32 4Kn existente é preservado.
- Extração Windows usa snapshot privado integral da ISO, incluído no custo temporário. Mudança detectada de identidade ou metadados durante a captura aborta antes de gravar; alterações posteriores na origem não mudam os bytes usados. O snapshot não comprova autenticidade nem garante detectar adulterações adversariais durante a captura.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: O sistema MUST reconhecer candidatos Windows por `sources/install.wim|esd|swm`, mesmo sem outros marcadores, e apresentar a arquitetura detectada ou a impossibilidade de determiná-la.
- **FR-002**: O sistema MUST criar mídia de instalação Windows 10/11 UEFI a partir das imagens da matriz declarada, preservando `bootmgr`, BCD, `EFI/Microsoft/Boot` e demais arquivos necessários existentes na origem.
- **FR-003**: O sistema MUST garantir o fallback removível correspondente à arquitetura: `EFI/BOOT/BOOTX64.EFI` para x64 ou `EFI/BOOT/BOOTAA64.EFI` para ARM64; deve preservar um fallback existente e recusar a imagem se não puder obter um fallback compatível da própria origem.
- **FR-004**: O sistema MUST dividir WIM acima do limite FAT32 quando produzir partes válidas, preservando imagens, edições e metadados necessários à instalação. Quando um recurso indivisível ou formato incompatível impedir a divisão, a seleção automática MUST usar NTFS e informar o motivo; FAT32 explicitamente solicitado MUST falhar antes da gravação.
- **FR-005**: O sistema MUST preservar ESD e conjuntos SWM válidos que caibam no destino sem transformações desnecessárias, recusando entradas fora dos limites documentados antes de gravar.
- **FR-006**: O sistema MUST validar legibilidade da origem, cadeia de boot, tamanho dos arquivos, capacidade do destino, espaço de preparação e disponibilidade dos recursos necessários antes de modificar a tabela de partições.
- **FR-007**: O sistema MUST manter as proteções existentes: bloqueio de disco de sistema, elegibilidade do destino, revalidação de identidade, confirmação explícita e proteção da ISO original.
- **FR-008**: CLI e GUI MUST permitir criação Windows pelo fluxo existente, mostrar o tratamento de arquivos grandes e substituir a rejeição genérica atual por suporte ou motivo específico de incompatibilidade. O modo automático deve selecionar extração para as ISOs Windows não híbridas suportadas.
- **FR-009**: CLI e GUI MUST oferecer opções Windows 11 independentes para TPM, Secure Boot e conta local, todas desativadas por padrão; o plano deve listar exatamente as alterações solicitadas e avisar sobre instalação fora dos requisitos oficiais.
- **FR-010**: O sistema MUST aplicar personalizações somente à mídia de destino, nunca ao hospedeiro ou à ISO, sem modificar os executáveis de boot assinados; combinações não suportadas e conflitos devem falhar antes da gravação.
- **FR-011**: O sistema MUST apresentar progresso durante preparação, cópia e verificação, aceitar cancelamento entre unidades de trabalho e informar explicitamente falhas ou destino incompleto. A documentação deve declarar qualquer etapa indivisível de cancelamento.
- **FR-012**: Com verificação solicitada, o sistema MUST detectar divergências nos arquivos copiados, partes geradas e personalizações; sem verificação, não deve alegar que houve releitura verificada.
- **FR-013**: A documentação MUST informar formatos aceitos, limitações, espaço temporário, opções, compatibilidade por arquitetura e situação real dos testes; deve registrar licenças, redistribuição e disponibilidade de fontes dos componentes entregues.
- **FR-014**: A conclusão de M5 MUST incluir evidências de Windows 10/11 x64 chegando à interface de instalação em ambiente UEFI virtualizado e em hardware UEFI físico; Windows 11 ARM64 deve ter evidência quando houver imagem e ambiente disponíveis, ou pendência explícita sem alegação de validação.
- **FR-015**: O suporte Windows MUST manter os fluxos DD e extração Linux existentes, inclusive MBR/GPT, verificação e regras de segurança, sem regressões nos testes existentes.
- **FR-016**: O sistema MUST oferecer neste M5 tanto FAT32 com WIM dividido quanto NTFS com UEFI:NTFS; o plano deve identificar o caminho utilizado e os arquivos que serão transformados ou preservados.
- **FR-017**: O caminho NTFS MUST preservar arquivos acima do limite FAT32, inclusive ESD, e iniciar o instalador por UEFI:NTFS na arquitetura declarada; componentes adicionais devem ter origem, licença e compatibilidade documentadas.
- **FR-018**: CLI e GUI MUST oferecer separadamente a opção de ignorar a exigência de Secure Boot do instalador Windows 11 e o acesso a orientações para desativação manual no firmware quando necessária. Ambas devem estar desativadas por padrão; nenhuma pode alterar automaticamente o firmware. As orientações devem avisar sobre a redução da proteção de boot, a possível solicitação da chave de recuperação BitLocker e a necessidade de confirmar sua disponibilidade antes de alterar o firmware.
- **FR-019**: CLI e GUI MUST selecionar automaticamente FAT32, com divisão de WIM quando necessária, usando NTFS com UEFI:NTFS quando houver arquivos acima do limite FAT32 não divisíveis pelo fluxo suportado, como ESD. O usuário deve poder escolher manualmente qualquer caminho; uma escolha incompatível deve ser recusada antes da gravação, sem troca silenciosa. O plano deve mostrar a escolha e o motivo.
- **FR-020**: A conclusão do M5 MUST incluir validação dos caminhos FAT32 e NTFS com UEFI:NTFS, com Secure Boot ativado e desativado, em firmware compatível com as assinaturas distribuídas. As evidências devem identificar firmware, estado do Secure Boot, assinaturas aceitas e versões dos componentes de boot; incompatibilidades conhecidas devem ser informadas antes da gravação, sem promessa de compatibilidade universal.

### Key Entities *(include if feature involves data)*

- **Imagem de instalação**: ISO de origem; identificação Windows, versão quando identificável, arquitetura, arquivos de boot e conteúdo de instalação. Permanece inalterada.
- **Conteúdo de instalação**: WIM, ESD ou conjunto SWM; tamanho, imagens/edições disponíveis, sequência de partes e necessidade de divisão.
- **Plano de criação**: Origem, identidade e capacidade do destino, esquema de partições, tratamento de arquivos grandes, necessidades temporárias, verificação e opções Windows 11 selecionadas.
- **Resultado de criação**: Estado concluído, cancelado ou falho; arquivos transformados, resultado da verificação e aviso de destino incompleto quando aplicável.
- **Evidência de compatibilidade**: Versão e arquitetura Windows, identificação da ISO, ambiente/firmware, revisão do projeto, opções usadas, procedimento e resultado observado.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Windows 10 x64 e Windows 11 x64 chegam à primeira tela do instalador, a partir de mídia gerada, em ambiente UEFI virtualizado e em pelo menos uma máquina UEFI física compatível; resultados e passos são registrados por versão.
- **SC-002**: Pelo menos uma imagem com WIM acima de 4 GiB permite ao instalador listar as mesmas edições da origem e iniciar a aplicação de uma edição em disco descartável de teste, sem erro de parte ausente ou ilegível.
- **SC-003**: Em 100% dos cenários negativos de pré-processamento documentados, o destino permanece byte a byte inalterado; a mensagem identifica a causa e uma próxima ação possível.
- **SC-004**: Em 100% dos testes sem opções Windows 11 selecionadas, não há personalização adicional da instalação; cada opção testada isoladamente altera apenas o comportamento solicitado.
- **SC-005**: O usuário consegue criar uma mídia Windows suportada tanto pela CLI quanto pela GUI sem dividir arquivos ou copiar arquivos de boot manualmente.
- **SC-006**: Toda corrupção introduzida nos casos de teste de verificação é detectada e nenhuma operação cancelada ou falha é anunciada como concluída.
- **SC-007**: A matriz de compatibilidade distingue para cada combinação declarada: validada, não validada ou não suportada, sem registrar ausência de hardware como sucesso.
- **SC-008**: Os dois caminhos, FAT32 com WIM dividido e NTFS com UEFI:NTFS, chegam à interface de instalação com Windows 10/11 x64 em ambiente UEFI virtualizado e hardware UEFI físico, com Secure Boot ativado e desativado em firmware compatível com as assinaturas distribuídas; pelo menos uma imagem com arquivo acima do limite FAT32 é validada no caminho NTFS sem divisão desse arquivo.
- **SC-009**: Nos testes de CLI e GUI, as duas opções relacionadas a Secure Boot aparecem separadamente e desativadas por padrão; consultar orientações não altera arquivos da mídia, configurações do hospedeiro ou firmware, e ignorar a exigência do instalador não é apresentado como desativação do firmware.

## Assumptions

- Ordem de entrega aprovada: implementar e validar primeiro o host macOS. Builds e validações dos hosts Linux/Windows serão executados separadamente pelo usuário; sua ausência não bloqueia o incremento macOS. Não anunciar esses hosts como validados antes das evidências. Isso não muda o alvo da mídia: instalação Windows UEFI.
- O escopo deriva de M5 em `ROADMAP.md`; os requisitos de segurança, documentação e evidências desse documento continuam aplicáveis.
- M5 inclui FAT32 com divisão de WIM e NTFS com UEFI:NTFS. O layout necessário ao boot NTFS faz parte desta entrega; a seleção automática prioriza FAT32 e usa NTFS para arquivos grandes não divisíveis pelo fluxo suportado, com escolha manual disponível.
- UEFI é o alvo Windows deste marco. Boot legado Windows/BOOTMGR em BIOS não faz parte desta entrega; o suporte Linux BIOS existente é preservado.
- A matriz obrigatória é Windows 10/11 x64. Windows 11 ARM64 é incluído quando houver imagens e ambientes legalmente disponíveis; Windows 10 ARM64 não é requisito obrigatório sem disponibilidade documentada.
- A matriz mínima usa Windows 10 22H2 x64 e Windows 11 24H2 x64. Cada ISO utilizada deve ter edição, build e SHA-256 registrados. Ajustes Windows 11 só são anunciados após validação na build exata; disponibilidade de ISO/ambiente é pré-requisito, não evidência de sucesso.
- macOS, Linux e Windows continuam sendo plataformas de criação; não se exige que o usuário instale ferramentas externas manualmente. A forma de entregar os recursos de preparação e cumprir suas licenças será definida no plano, respeitando a integração wimlib solicitada pelo roadmap.
- O usuário fornece ISOs obtidas legalmente; download, ativação, licenças Windows e redistribuição de imagens Microsoft não fazem parte do produto.
- Os ajustes Windows 11 modificam apenas a experiência do instalador compatível; não desativam TPM ou Secure Boot no firmware, nem garantem suporte oficial da Microsoft a hardware incompatível.
- Arquivos grandes não-WIM, inclusive ESD, são preservados no caminho NTFS; conversão de ESD não é necessária para este escopo.
- Validação física exige um USB descartável e autorização específica para o dispositivo. Nenhuma gravação física é autorizada por esta especificação; o Expansion HDD citado no roadmap não deve ser usado.
- `.specify/memory/constitution.md` contém apenas o modelo não preenchido; não acrescenta princípios ratificados.
