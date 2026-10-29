#include "result_illustration_p.h"

#include <QPainter>
#include <QPainterPath>
#include <QPen>

#include <algorithm>
#include <cmath>

namespace adqt::widgets::detail {
namespace {

// These compositions are drawn for Qt, rather than converted from SVG. Keeping their paths in
// logical coordinates makes the same geometry useful on fractional-scale and high-DPI displays.
struct Artwork {
  QPainterPath clouds;
  QPainterPath network;
  QPainterPath rightNetwork;
  QPainterPath trousers;
  QPainterPath trouserShade;
  QPainterPath trouserSeams;
  QPainterPath leftShoe;
  QPainterPath rightShoe;
  QPainterPath soles;
  QPainterPath neck;
  QPainterPath face;
  QPainterPath hair;
  QPainterPath faceDetails;
  QPainterPath leftRaisedArm;
  QPainterPath rightRaisedArm;
  QPainterPath leftRaisedArmDetails;
  QPainterPath rightRaisedArmDetails;
  QPainterPath raisedShirt;
  QPainterPath raisedShirtDetails;
  QPainterPath holdingShirt;
  QPainterPath holdingShirtDetails;
  QPainterPath holdingArm;
  QPainterPath holdingHand;
  QPainterPath holdingHandDetails;
  QPainterPath thinkingShirt;
  QPainterPath thinkingShirtDetails;
  QPainterPath foldedArm;
  QPainterPath thinkingArm;
  QPainterPath thinkingArmDetails;

  Artwork() {
    clouds.setFillRule(Qt::WindingFill);
    clouds.addRoundedRect(QRectF(95, 19, 38, 9), 4.5, 4.5);
    clouds.addRoundedRect(QRectF(106, 37, 39, 9), 4.5, 4.5);
    clouds.moveTo(113, 27);
    clouds.lineTo(128, 27);
    clouds.cubicTo(121, 28, 121, 36, 128, 38);
    clouds.lineTo(113, 38);
    clouds.cubicTo(120, 36, 120, 28, 113, 27);
    clouds.closeSubpath();
    clouds.addRoundedRect(QRectF(157, 58, 13, 9), 4.5, 4.5);

    network.moveTo(32, 131);
    network.lineTo(52, 143);
    network.lineTo(34, 158);
    rightNetwork.moveTo(175, 108);
    rightNetwork.lineTo(175, 135);
    rightNetwork.lineTo(196, 153);
    rightNetwork.lineTo(178, 170);
    rightNetwork.moveTo(192, 93);
    rightNetwork.lineTo(192, 117);
    rightNetwork.lineTo(208, 125);
    rightNetwork.lineTo(222, 117);
    rightNetwork.moveTo(214, 154);
    rightNetwork.lineTo(212, 175);
    rightNetwork.lineTo(200, 182);
    rightNetwork.moveTo(176, 173);
    rightNetwork.lineTo(166, 185);
    rightNetwork.lineTo(156, 185);
    rightNetwork.lineTo(150, 193);

    trousers.moveTo(80, 148);
    trousers.cubicTo(91, 146, 106, 147, 117, 149);
    trousers.cubicTo(118, 171, 115, 194, 114, 215);
    trousers.lineTo(114, 278);
    trousers.lineTo(100, 278);
    trousers.lineTo(99, 218);
    trousers.lineTo(98, 177);
    trousers.cubicTo(92, 196, 88, 215, 81, 234);
    trousers.lineTo(68, 278);
    trousers.lineTo(53, 278);
    trousers.lineTo(66, 226);
    trousers.cubicTo(72, 198, 77, 169, 80, 148);
    trousers.closeSubpath();

    trouserShade.moveTo(80, 151);
    trouserShade.lineTo(84, 152);
    trouserShade.cubicTo(80, 179, 76, 204, 70, 227);
    trouserShade.lineTo(57, 277);
    trouserShade.lineTo(53, 277);
    trouserShade.lineTo(66, 225);
    trouserShade.closeSubpath();

    trouserSeams.moveTo(84, 152);
    trouserSeams.lineTo(83, 163);
    trouserSeams.lineTo(80, 165);
    trouserSeams.moveTo(101, 151);
    trouserSeams.lineTo(100, 173);
    trouserSeams.lineTo(94, 176);
    trouserSeams.moveTo(105, 151);
    trouserSeams.lineTo(104, 164);
    trouserSeams.lineTo(100, 167);
    trouserSeams.moveTo(100, 176);
    trouserSeams.lineTo(102, 218);
    trouserSeams.lineTo(104, 222);
    trouserSeams.lineTo(102, 226);
    trouserSeams.lineTo(103, 273);
    trouserSeams.moveTo(78, 176);
    trouserSeams.lineTo(72, 209);
    trouserSeams.moveTo(71, 216);
    trouserSeams.lineTo(69, 226);
    trouserSeams.lineTo(60, 259);
    trouserSeams.moveTo(74, 224);
    trouserSeams.quadTo(80, 223, 83, 219);
    trouserSeams.moveTo(76, 220);
    trouserSeams.lineTo(82, 216);
    trouserSeams.moveTo(106, 222);
    trouserSeams.lineTo(112, 219);
    trouserSeams.moveTo(106, 225);
    trouserSeams.lineTo(111, 223);
    trouserSeams.moveTo(54, 274);
    trouserSeams.lineTo(68, 274);
    trouserSeams.moveTo(101, 274);
    trouserSeams.lineTo(113, 274);

    leftShoe.moveTo(55, 277);
    leftShoe.lineTo(66, 277);
    leftShoe.lineTo(66, 284);
    leftShoe.quadTo(70, 289, 67, 293);
    leftShoe.quadTo(58, 295, 50, 293);
    leftShoe.quadTo(50, 289, 53, 284);
    leftShoe.closeSubpath();
    rightShoe.moveTo(101, 277);
    rightShoe.lineTo(113, 277);
    rightShoe.quadTo(114, 284, 122, 286);
    rightShoe.quadTo(129, 287, 128, 292);
    rightShoe.quadTo(115, 295, 101, 292);
    rightShoe.closeSubpath();
    soles.moveTo(50, 291);
    soles.quadTo(59, 294, 68, 291);
    soles.lineTo(67, 294);
    soles.quadTo(58, 296, 50, 294);
    soles.closeSubpath();
    soles.moveTo(101, 291);
    soles.quadTo(114, 294, 128, 290);
    soles.lineTo(128, 293);
    soles.quadTo(114, 296, 101, 294);
    soles.closeSubpath();

    neck.moveTo(84, 77);
    neck.lineTo(97, 81);
    neck.lineTo(96, 91);
    neck.quadTo(88, 96, 79, 88);
    neck.closeSubpath();
    face.moveTo(84, 64);
    face.cubicTo(89, 60, 100, 61, 104, 66);
    face.lineTo(102, 77);
    face.cubicTo(101, 85, 99, 89, 94, 89);
    face.cubicTo(89, 88, 85, 84, 83, 80);
    face.cubicTo(78, 79, 77, 74, 79, 72);
    face.quadTo(81, 70, 84, 73);
    face.closeSubpath();
    hair.moveTo(82, 79);
    hair.cubicTo(77, 74, 75, 66, 79, 61);
    hair.cubicTo(82, 56, 86, 55, 90, 56);
    hair.cubicTo(96, 57, 100, 60, 106, 60);
    hair.quadTo(106, 67, 102, 72);
    hair.lineTo(102, 67);
    hair.quadTo(93, 66, 88, 63);
    hair.lineTo(85, 72);
    hair.quadTo(81, 69, 80, 73);
    hair.quadTo(80, 76, 83, 77);
    hair.closeSubpath();
    faceDetails.moveTo(91, 71);
    faceDetails.lineTo(94, 72);
    faceDetails.moveTo(99, 73);
    faceDetails.lineTo(102, 74);
    faceDetails.moveTo(97, 75);
    faceDetails.lineTo(96, 80);
    faceDetails.lineTo(98, 81);
    faceDetails.moveTo(91, 83);
    faceDetails.quadTo(94, 85, 97, 84);
    faceDetails.moveTo(81, 74);
    faceDetails.lineTo(82, 76);

    leftRaisedArm.moveTo(69, 85);
    leftRaisedArm.lineTo(77, 92);
    leftRaisedArm.quadTo(68, 99, 60, 96);
    leftRaisedArm.lineTo(47, 91);
    leftRaisedArm.cubicTo(40, 88, 42, 81, 48, 77);
    leftRaisedArm.lineTo(81, 60);
    leftRaisedArm.quadTo(86, 58, 91, 61);
    leftRaisedArm.lineTo(88, 65);
    leftRaisedArm.lineTo(80, 66);
    leftRaisedArm.closeSubpath();
    rightRaisedArm.moveTo(104, 62);
    rightRaisedArm.lineTo(139, 76);
    rightRaisedArm.cubicTo(149, 80, 151, 86, 145, 91);
    rightRaisedArm.quadTo(141, 96, 133, 92);
    rightRaisedArm.lineTo(117, 86);
    rightRaisedArm.lineTo(124, 80);
    rightRaisedArm.lineTo(106, 68);
    rightRaisedArm.lineTo(102, 66);
    rightRaisedArm.closeSubpath();
    leftRaisedArmDetails.moveTo(46, 89);
    leftRaisedArmDetails.quadTo(57, 89, 64, 79);
    rightRaisedArmDetails.moveTo(132, 82);
    rightRaisedArmDetails.quadTo(136, 89, 144, 91);

    raisedShirt.moveTo(67, 85);
    raisedShirt.lineTo(80, 88);
    raisedShirt.quadTo(93, 97, 107, 89);
    raisedShirt.lineTo(126, 86);
    raisedShirt.lineTo(135, 99);
    raisedShirt.lineTo(120, 107);
    raisedShirt.lineTo(118, 147);
    raisedShirt.quadTo(98, 145, 79, 149);
    raisedShirt.lineTo(74, 109);
    raisedShirt.lineTo(57, 100);
    raisedShirt.quadTo(65, 94, 67, 85);
    raisedShirt.closeSubpath();
    raisedShirtDetails.moveTo(79, 90);
    raisedShirtDetails.quadTo(92, 101, 108, 92);
    raisedShirtDetails.moveTo(75, 95);
    raisedShirtDetails.quadTo(76, 103, 71, 108);
    raisedShirtDetails.moveTo(127, 90);
    raisedShirtDetails.lineTo(131, 100);
    raisedShirtDetails.moveTo(87, 116);
    raisedShirtDetails.lineTo(103, 113);
    raisedShirtDetails.moveTo(100, 116);
    raisedShirtDetails.lineTo(110, 114);

    holdingShirt.moveTo(67, 85);
    holdingShirt.lineTo(81, 88);
    holdingShirt.quadTo(92, 95, 104, 90);
    holdingShirt.cubicTo(116, 92, 122, 100, 125, 111);
    holdingShirt.lineTo(117, 119);
    holdingShirt.lineTo(118, 147);
    holdingShirt.quadTo(98, 145, 79, 149);
    holdingShirt.lineTo(74, 108);
    holdingShirt.lineTo(57, 100);
    holdingShirt.quadTo(65, 94, 67, 85);
    holdingShirt.closeSubpath();
    holdingShirtDetails.moveTo(80, 90);
    holdingShirtDetails.quadTo(91, 99, 104, 93);
    holdingShirtDetails.moveTo(74, 95);
    holdingShirtDetails.quadTo(75, 101, 71, 107);
    holdingShirtDetails.moveTo(116, 106);
    holdingShirtDetails.lineTo(117, 117);
    holdingArm.moveTo(119, 109);
    holdingArm.quadTo(129, 118, 129, 132);
    holdingArm.lineTo(121, 138);
    holdingArm.lineTo(115, 120);
    holdingArm.closeSubpath();
    holdingHand.moveTo(151, 131);
    holdingHand.quadTo(158, 133, 158, 139);
    holdingHand.quadTo(157, 146, 152, 146);
    holdingHand.quadTo(147, 146, 145, 141);
    holdingHand.lineTo(143, 136);
    holdingHand.quadTo(143, 134, 147, 135);
    holdingHand.lineTo(151, 135);
    holdingHand.closeSubpath();
    holdingHandDetails.moveTo(145, 137);
    holdingHandDetails.quadTo(151, 139, 155, 138);
    holdingHandDetails.moveTo(146, 140);
    holdingHandDetails.quadTo(151, 142, 155, 141);
    holdingHandDetails.moveTo(148, 143);
    holdingHandDetails.lineTo(153, 144);

    thinkingShirt.moveTo(79, 87);
    thinkingShirt.quadTo(91, 95, 104, 90);
    thinkingShirt.quadTo(116, 91, 122, 108);
    thinkingShirt.lineTo(113, 115);
    thinkingShirt.lineTo(117, 146);
    thinkingShirt.quadTo(98, 143, 80, 147);
    thinkingShirt.lineTo(75, 121);
    thinkingShirt.lineTo(70, 118);
    thinkingShirt.quadTo(64, 103, 68, 94);
    thinkingShirt.quadTo(71, 89, 79, 87);
    thinkingShirt.closeSubpath();
    thinkingShirtDetails.moveTo(80, 90);
    thinkingShirtDetails.quadTo(91, 99, 104, 93);
    thinkingShirtDetails.moveTo(79, 99);
    thinkingShirtDetails.lineTo(83, 107);
    thinkingShirtDetails.moveTo(75, 119);
    thinkingShirtDetails.quadTo(78, 114, 84, 111);
    thinkingShirtDetails.moveTo(113, 106);
    thinkingShirtDetails.lineTo(114, 116);
    foldedArm.moveTo(119, 111);
    foldedArm.quadTo(123, 117, 126, 129);
    foldedArm.quadTo(129, 137, 121, 139);
    foldedArm.quadTo(104, 140, 90, 134);
    foldedArm.lineTo(79, 133);
    foldedArm.quadTo(73, 131, 75, 128);
    foldedArm.lineTo(87, 124);
    foldedArm.quadTo(106, 129, 118, 126);
    foldedArm.lineTo(114, 115);
    foldedArm.closeSubpath();
    thinkingArm.moveTo(75, 122);
    thinkingArm.quadTo(81, 110, 88, 99);
    thinkingArm.lineTo(91, 90);
    thinkingArm.lineTo(88, 84);
    thinkingArm.quadTo(86, 80, 89, 80);
    thinkingArm.lineTo(95, 85);
    thinkingArm.quadTo(99, 87, 97, 93);
    thinkingArm.lineTo(92, 109);
    thinkingArm.lineTo(86, 126);
    thinkingArm.quadTo(82, 132, 77, 129);
    thinkingArm.quadTo(73, 127, 75, 122);
    thinkingArm.closeSubpath();
    thinkingArmDetails.moveTo(89, 85);
    thinkingArmDetails.lineTo(93, 89);
    thinkingArmDetails.lineTo(92, 94);
    thinkingArmDetails.moveTo(79, 125);
    thinkingArmDetails.quadTo(82, 125, 85, 126);
    thinkingArmDetails.moveTo(84, 132);
    thinkingArmDetails.lineTo(90, 131);
    thinkingArmDetails.moveTo(112, 126);
    thinkingArmDetails.quadTo(119, 124, 122, 126);
  }
};

const Artwork& artwork() {
  static const Artwork value;
  return value;
}

QColor color(QRgb value) { return QColor::fromRgb(value); }

void fill(QPainter* painter, const QPainterPath& path, QRgb value) {
  painter->fillPath(path, color(value));
}

void stroke(QPainter* painter, const QPainterPath& path, QRgb value, qreal width = 1.0) {
  painter->strokePath(path, QPen(color(value), width, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
}

void circle(QPainter* painter, qreal x, qreal y, qreal radius, QRgb value) {
  painter->setBrush(color(value));
  painter->setPen(Qt::NoPen);
  painter->drawEllipse(QPointF(x, y), radius, radius);
}

void roundRect(QPainter* painter, const QRectF& rect, qreal radius, QRgb value) {
  painter->setBrush(color(value));
  painter->setPen(Qt::NoPen);
  painter->drawRoundedRect(rect, radius, radius);
}

void paintBackdrop(QPainter* painter, bool withRightNetwork) {
  const auto& paths = artwork();
  circle(painter, 125, 127, 125, 0xe4ebf7);
  fill(painter, paths.clouds, 0xffffff);
  stroke(painter, paths.network, 0xffffff, 2);
  circle(painter, 32, 131, 8, 0xffffff);
  circle(painter, 52, 143, 5.5, 0xffffff);
  circle(painter, 34, 158, 5.5, 0xffffff);
  if (!withRightNetwork) return;
  stroke(painter, paths.rightNetwork, 0xffffff, 2);
  circle(painter, 175, 135, 9.5, 0xffffff);
  circle(painter, 196, 153, 6.5, 0xffffff);
  circle(painter, 178, 170, 6.5, 0xffffff);
  painter->setPen(QPen(Qt::white, 1.8));
  painter->setBrush(color(0xe4ebf7));
  for (const QPointF point : {QPointF(175, 108), QPointF(192, 93), QPointF(222, 117),
                              QPointF(214, 154), QPointF(200, 182), QPointF(150, 193)}) {
    painter->drawEllipse(point, 2.3, 2.3);
  }
}

void paintBadge(QPainter* painter, int status, const QColor& primary) {
  painter->setPen(Qt::NoPen);
  painter->setBrush(status == 403 ? color(0xa26cf4) : (status == 500 ? color(0xff603b) : primary));
  painter->drawEllipse(QPointF(206, 31), 29.5, 29.5);
  painter->setPen(QPen(Qt::white, 3.5, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
  painter->setBrush(Qt::NoBrush);
  if (status == 403) {
    painter->drawRoundedRect(QRectF(200, 18, 12, 16), 6, 6);
    roundRect(painter, QRectF(194, 27, 24, 16), 1.8, 0xffffff);
  } else if (status == 500) {
    painter->drawLine(QPointF(206, 19), QPointF(206, 33));
    circle(painter, 206, 41, 2.5, 0xffffff);
  } else {
    static const QPainterPath question = [] {
      QPainterPath path;
      path.moveTo(200, 23);
      path.cubicTo(200, 16, 212, 16, 212, 23);
      path.cubicTo(212, 28, 206, 29, 206, 35);
      return path;
    }();
    painter->drawPath(question);
    circle(painter, 206, 41, 2.5, 0xffffff);
  }
}

void paintLegs(QPainter* painter) {
  const auto& paths = artwork();
  fill(painter, paths.leftShoe, 0xcbd2d6);
  fill(painter, paths.rightShoe, 0xcbd2d6);
  fill(painter, paths.soles, 0x552052);
  roundRect(painter, QRectF(56, 277, 4, 5.5), 1.5, 0xa9b3bf);
  roundRect(painter, QRectF(103, 277, 4, 5), 1.5, 0xa9b3bf);
  fill(painter, paths.trousers, 0x7ab1f6);
  fill(painter, paths.trouserShade, 0x68a1e8);
  stroke(painter, paths.trouserSeams, 0x5f8fcc, 1.05);
  painter->setPen(QPen(color(0x192a67), 4));
  painter->drawLine(QPointF(81, 150), QPointF(116, 149));
  roundRect(painter, QRectF(97, 146, 9, 6), 0.5, 0xffffff);
  roundRect(painter, QRectF(98.5, 147.3, 6, 3.4), 0.3, 0x192a67);
}

void paintHead(QPainter* painter) {
  const auto& paths = artwork();
  fill(painter, paths.neck, 0xffb695);
  fill(painter, paths.face, 0xffc8a4);
  fill(painter, paths.hair, 0x530b43);
  stroke(painter, paths.faceDetails, 0xc97865, 1.1);
  circle(painter, 92, 75, 0.8, 0x61334e);
  circle(painter, 100, 77, 0.8, 0x61334e);
}

void paintPackage(QPainter* painter) {
  painter->setPen(Qt::NoPen);
  painter->setBrush(color(0xf2d5a5));
  painter->drawRect(QRectF(98, 127, 53, 25));
  painter->setBrush(color(0xeac291));
  painter->drawRect(QRectF(129, 127, 22, 25));
  static const QPainterPath flaps = [] {
    QPainterPath path;
    path.moveTo(98, 118);
    path.lineTo(129, 118);
    path.lineTo(121, 128);
    path.lineTo(91, 128);
    path.closeSubpath();
    path.moveTo(129, 118);
    path.lineTo(149, 118);
    path.lineTo(157, 128);
    path.lineTo(137, 128);
    path.closeSubpath();
    return path;
  }();
  fill(painter, flaps, 0xf3d4a0);
  painter->setPen(QPen(color(0xc99060), 0.8, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
  painter->drawLine(QPointF(129, 119), QPointF(129, 151));
  painter->drawLine(QPointF(109, 136), QPointF(106, 139));
  painter->drawLine(QPointF(106, 139), QPointF(109, 142));
  painter->drawLine(QPointF(114, 136), QPointF(112, 142));
  painter->drawLine(QPointF(117, 136), QPointF(120, 139));
  painter->drawLine(QPointF(120, 139), QPointF(117, 142));
}

void paintPerson(QPainter* painter, int status) {
  const auto& paths = artwork();
  painter->save();
  if (status == 500) painter->translate(-8, 0);
  // Raised arms sit behind the sleeves; the crossed forearms sit in front of the torso.
  if (status != 500) {
    fill(painter, paths.leftRaisedArm, 0xffc5a0);
    stroke(painter, paths.leftRaisedArmDetails, 0xdf957b, 1.05);
    if (status == 403) {
      fill(painter, paths.rightRaisedArm, 0xffc5a0);
      stroke(painter, paths.rightRaisedArmDetails, 0xdf957b, 1.05);
    }
  }
  if (status == 403) {
    fill(painter, paths.raisedShirt, 0xffffff);
    stroke(painter, paths.raisedShirtDetails, 0xe0e9f7, 1.1);
  } else if (status == 404) {
    fill(painter, paths.holdingArm, 0xffb695);
    fill(painter, paths.holdingShirt, 0xffffff);
    stroke(painter, paths.holdingShirtDetails, 0xe0e9f7, 1.1);
  } else {
    fill(painter, paths.thinkingShirt, 0xffffff);
    stroke(painter, paths.thinkingShirtDetails, 0xe0e9f7, 1.1);
  }
  paintLegs(painter);
  paintHead(painter);
  if (status == 404) {
    paintPackage(painter);
    fill(painter, paths.holdingHand, 0xffbf9c);
    stroke(painter, paths.holdingHandDetails, 0xda8b72, 1.1);
  } else if (status == 500) {
    fill(painter, paths.foldedArm, 0xffc5a0);
    fill(painter, paths.thinkingArm, 0xffc5a0);
    stroke(painter, paths.thinkingArmDetails, 0xdc9278, 1.05);
  }
  painter->restore();
}

void paintPlant(QPainter* painter, bool small) {
  if (small) {
    roundRect(painter, QRectF(155, 237, 19, 33), 9.5, 0x5da42e);
    roundRect(painter, QRectF(164, 247, 19, 23), 9.5, 0x91c30f);
    roundRect(painter, QRectF(151, 268, 36, 27), 3, 0xf1d6a9);
  } else {
    roundRect(painter, QRectF(161, 170, 13, 77), 6.5, 0x5ca12d);
    roundRect(painter, QRectF(172, 202, 14, 45), 7, 0x8ec211);
    roundRect(painter, QRectF(155, 242, 34, 53), 3.5, 0xf1d6a9);
  }
}

void paintServers(QPainter* painter) {
  roundRect(painter, QRectF(184, 187, 9, 25), 1, 0x9eb3c6);
  roundRect(painter, QRectF(141, 210, 96, 10), 5, 0x9eb3c6);
  roundRect(painter, QRectF(173, 206, 32, 16), 2.5, 0xbdcddd);
  painter->setPen(QPen(color(0xa5b8cb), 1.2));
  painter->drawLine(QPointF(177, 209), QPointF(177, 219));
  painter->drawLine(QPointF(201, 209), QPointF(201, 219));
  for (int index = 0; index < 3; ++index) {
    const qreal y = 107 + index * 30;
    roundRect(painter, QRectF(154, y + 20, 68, 10), 2, 0xa9bbce);
    roundRect(painter, QRectF(150, y, 77, 25), 3, 0xbdcddd);
    for (int light = 0; light < 3; ++light) {
      circle(painter, 159 + light * 7.5, y + 12.5, 2.3, 0xffffff);
    }
    roundRect(painter, QRectF(199, y + 10, 24, 5), 0.8, 0xffffff);
    painter->setPen(QPen(color(0x8199af), 1.05, Qt::SolidLine, Qt::RoundCap));
    painter->drawLine(QPointF(155, y + 25), QPointF(162, y + 25));
    painter->drawLine(QPointF(166, y + 25), QPointF(171, y + 25));
    painter->drawLine(QPointF(175, y + 25), QPointF(222, y + 25));
  }
  painter->setPen(QPen(color(0xbdcddd), 1.8, Qt::SolidLine, Qt::RoundCap));
  for (const QPointF center : {QPointF(191, 86), QPointF(127, 165)}) {
    painter->drawLine(center + QPointF(-5, -5), center + QPointF(5, 5));
    painter->drawLine(center + QPointF(-5, 5), center + QPointF(5, -5));
  }
}

}  // namespace

void paintResultIllustration(QPainter* painter, const QRectF& rect, int status,
                             const QColor& primary) {
  if (!painter || !painter->isActive() || !rect.isValid() || !std::isfinite(rect.x()) ||
      !std::isfinite(rect.y()) || !std::isfinite(rect.width()) || !std::isfinite(rect.height())) {
    return;
  }
  const qreal scale = std::min(rect.width() / 250.0, rect.height() / 295.0);
  if (status != 403 && status != 500) status = 404;
  painter->save();
  painter->setRenderHint(QPainter::Antialiasing);
  painter->translate(rect.center().x() - 125 * scale, rect.center().y() - 147.5 * scale);
  painter->scale(scale, scale);
  painter->setPen(Qt::NoPen);
  paintBackdrop(painter, status != 500);
  paintBadge(painter, status, primary.isValid() ? primary : color(0x1677ff));
  if (status == 500) paintServers(painter);
  paintPerson(painter, status);
  paintPlant(painter, status == 500);
  painter->restore();
}

}  // namespace adqt::widgets::detail
