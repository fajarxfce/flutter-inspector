# Geolocator consumer fixture

Independent Android app used to test GPS simulation. Uses geolocator's default fused provider, with no Inspector bridge or mock code in the app. Displays each Position as selectable JSON and logs `INSPECTOR_GPS_FIX` / `INSPECTOR_GPS_ERROR`.

```
flutter pub get
flutter build apk --debug --target-platform android-x64
adb -s emulator-5580 install -r -g build/app/outputs/flutter-apk/app-debug.apk
```

Use an isolated emulator. From the repository root run `python3 tools/gps_android_test.py emulator-5580`. The test starts the fixture and moves the emulator's GPS, then verifies restoration. Normal location permission and enabled device location are required.
