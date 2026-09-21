import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/pages/desktop_home_page.dart';
import 'package:flutter_hbb/desktop/pages/desktop_setting_page.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:get/get.dart';
import 'package:window_manager/window_manager.dart';
// import 'package:flutter/services.dart';

import '../../common/shared_state.dart';

class DesktopTabPage extends StatefulWidget {
  const DesktopTabPage({Key? key}) : super(key: key);

  @override
  State<DesktopTabPage> createState() => _DesktopTabPageState();

  static Future<bool> _verifySettingsPassword() async {
    final controller = TextEditingController();
    String errorText = '';

    final result = await gFFI.dialogManager.show<bool>(
      (setState, close, context) {
        Future<void> submit() async {
          final password = controller.text;
          if (password.isEmpty) {
            setState(() {
              errorText = translate('Password Required');
            });
            return;
          }

          final ok =
              await bind.mainVerifyManagedPassword(password: password);
          if (ok) {
            controller.clear();
            close(true);
          } else {
            controller.clear();
            setState(() {
              errorText = translate('Wrong Password');
            });
          }
        }

        return CustomAlertDialog(
          title: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(Icons.lock_outline, color: MyTheme.accent),
              Text(translate('Settings')).paddingOnly(left: 10),
            ],
          ),
          content: ConstrainedBox(
            constraints: const BoxConstraints(minWidth: 420),
            child: TextField(
              controller: controller,
              obscureText: true,
              autofocus: true,
              decoration: InputDecoration(
                labelText: translate('Password'),
                errorText: errorText.isEmpty ? null : errorText,
              ),
              onChanged: (_) {
                if (errorText.isNotEmpty) {
                  setState(() {
                    errorText = '';
                  });
                }
              },
              onSubmitted: (_) => submit(),
            ).workaroundFreezeLinuxMint(),
          ),
          actions: [
            dialogButton(
              'Cancel',
              onPressed: () => close(false),
              isOutline: true,
            ),
            dialogButton('OK', onPressed: submit),
          ],
          onSubmit: submit,
          onCancel: () => close(false),
        );
      },
      clickMaskDismiss: false,
      backDismiss: true,
      tag: 'managed-settings-password',
    );

    controller.dispose();
    return result == true;
  }

  static Future<void> onAddSetting(
      {SettingsTabKey initialPage = SettingsTabKey.general}) async {
    if (!await _verifySettingsPassword()) {
      return;
    }
    try {
      DesktopTabController tabController = Get.find<DesktopTabController>();
      tabController.add(TabInfo(
          key: kTabLabelSettingPage,
          label: kTabLabelSettingPage,
          selectedIcon: Icons.build_sharp,
          unselectedIcon: Icons.build_outlined,
          page: DesktopSettingPage(
            key: const ValueKey(kTabLabelSettingPage),
            initialTabkey: initialPage,
          )));
    } catch (e) {
      debugPrintStack(label: '$e');
    }
  }
}

class _DesktopTabPageState extends State<DesktopTabPage> {
  final tabController = DesktopTabController(tabType: DesktopTabType.main);

  _DesktopTabPageState() {
    RemoteCountState.init();
    Get.put<DesktopTabController>(tabController);
    tabController.add(TabInfo(
        key: kTabLabelHomePage,
        label: kTabLabelHomePage,
        selectedIcon: Icons.home_sharp,
        unselectedIcon: Icons.home_outlined,
        closable: false,
        page: DesktopHomePage(
          key: const ValueKey(kTabLabelHomePage),
        )));
    if (bind.isIncomingOnly()) {
      tabController.onSelected = (key) {
        if (key == kTabLabelHomePage) {
          windowManager.setSize(getIncomingOnlyHomeSize());
          setResizable(false);
        } else {
          windowManager.setSize(getIncomingOnlySettingsSize());
          setResizable(true);
        }
      };
    }
  }

  @override
  void initState() {
    super.initState();
    // HardwareKeyboard.instance.addHandler(_handleKeyEvent);
  }

  /*
  bool _handleKeyEvent(KeyEvent event) {
    if (!mouseIn && event is KeyDownEvent) {
      print('key down: ${event.logicalKey}');
      shouldBeBlocked(_block, canBeBlocked);
    }
    return false; // allow it to propagate
  }
  */

  @override
  void dispose() {
    // HardwareKeyboard.instance.removeHandler(_handleKeyEvent);
    Get.delete<DesktopTabController>();

    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tabWidget = Container(
        child: Scaffold(
            backgroundColor: Theme.of(context).colorScheme.background,
            body: DesktopTab(
              controller: tabController,
              tail: Offstage(
                offstage: bind.isIncomingOnly() || bind.isDisableSettings(),
                child: ActionIcon(
                  message: 'Settings',
                  icon: IconFont.menu,
                  onTap: () => DesktopTabPage.onAddSetting(),
                  isClose: false,
                ),
              ),
            )));
    return isMacOS || kUseCompatibleUiMode
        ? tabWidget
        : Obx(
            () => DragToResizeArea(
              resizeEdgeSize: stateGlobal.resizeEdgeSize.value,
              enableResizeEdges: windowManagerEnableResizeEdges,
              child: tabWidget,
            ),
          );
  }
}
